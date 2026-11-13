//! Request parsing, routing by path prefix, and bearer-token checks.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Response, StatusCode};

use crate::State;

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Bytes,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn form(&self) -> HashMap<String, String> {
        url::form_urlencoded::parse(&self.body).into_owned().collect()
    }

    /// `Range: bytes=N-` → N.
    pub fn range_start(&self) -> Option<u64> {
        self.header("range")?.strip_prefix("bytes=")?.split('-').next()?.parse().ok()
    }
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(status: StatusCode, value: &serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: value.to_string().into_bytes(),
        }
    }

    pub fn ok(value: &serde_json::Value) -> Self {
        Self::json(StatusCode::OK, value)
    }

    pub fn bytes(status: StatusCode, body: Vec<u8>) -> Self {
        Self { status, headers: vec![("content-type".into(), "application/octet-stream".into())], body }
    }

    pub fn empty(status: StatusCode) -> Self {
        Self { status, headers: Vec::new(), body: Vec::new() }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// The content from `start` on, as 206 when a range was asked for.
    pub fn content(content: &[u8], start: Option<u64>) -> Self {
        match start {
            None => Self::bytes(StatusCode::OK, content.to_vec()),
            Some(start) if usize::try_from(start).is_ok_and(|s| s >= content.len()) && !content.is_empty() => {
                Self::empty(StatusCode::RANGE_NOT_SATISFIABLE)
            }
            Some(start) => {
                let from = usize::try_from(start).unwrap_or(usize::MAX).min(content.len());
                let end = content.len().saturating_sub(1);
                Self::bytes(StatusCode::PARTIAL_CONTENT, content[from..].to_vec())
                    .with_header("content-range", &format!("bytes {from}-{end}/{}", content.len()))
            }
        }
    }
}

pub async fn handle(
    state: Arc<Mutex<State>>,
    request: hyper::Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let (parts, body) = request.into_parts();
    let body = body.collect().await.map(http_body_util::Collected::to_bytes).unwrap_or_default();
    let query =
        parts.uri.query().map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect()).unwrap_or_default();
    let headers = parts
        .headers
        .iter()
        .filter_map(|(name, value)| Some((name.as_str().to_ascii_lowercase(), value.to_str().ok()?.to_owned())))
        .collect();
    let request = Request { method: parts.method.to_string(), path: parts.uri.path().to_owned(), query, headers, body };
    let reply = {
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        route(&mut state, &request)
    };
    let mut response = Response::builder().status(reply.status);
    for (name, value) in &reply.headers {
        response = response.header(name, value);
    }
    Ok(response.body(Full::new(Bytes::from(reply.body))).unwrap_or_default())
}

fn route(state: &mut State, request: &Request) -> Reply {
    if request.path.ends_with("/v1/messages") || request.path.ends_with("/chat/completions") {
        return crate::models::handle(state, request);
    }
    if let Some(rest) = request.path.strip_prefix("/oauth/") {
        return crate::oauth::handle(state, rest, request);
    }
    if let Some(id) = request.path.strip_prefix("/graph/monitor/") {
        return crate::graph::monitor(state, id);
    }
    // Graph's upload and download URLs are pre-authorised: no bearer token.
    if let Some(rest) = request.path.strip_prefix("/graph/upload/").or(request.path.strip_prefix("/graph/download/")) {
        if request.header("authorization").is_some() && request.path.starts_with("/graph/upload/") {
            return Reply::json(
                StatusCode::UNAUTHORIZED,
                &serde_json::json!({"error": {"code": "unauthenticated", "message": "upload URLs take no Authorization header"}}),
            );
        }
        return crate::graph::handle_presigned(state, &request.path, rest, request);
    }
    if request.path.starts_with("/gdrive/upload/") && request.query.contains_key("upload_id") {
        return guarded(state, request, crate::google::handle_upload);
    }
    if let Some(rest) = request.path.strip_prefix("/gdrive/") {
        let rest = rest.to_owned();
        return guarded(state, request, move |state, request| crate::google::handle(state, &rest, request));
    }
    if let Some(rest) = request.path.strip_prefix("/dropbox/") {
        let rest = rest.to_owned();
        return guarded(state, request, move |state, request| crate::dropbox::handle(state, &rest, request));
    }
    if let Some(rest) = request.path.strip_prefix("/graph/v1.0/") {
        let rest = rest.to_owned();
        return guarded(state, request, move |state, request| crate::graph::handle(state, &rest, request));
    }
    Reply::empty(StatusCode::NOT_FOUND)
}

/// Rate limits and bearer tokens apply to every API call.
fn guarded(state: &mut State, request: &Request, handler: impl FnOnce(&mut State, &Request) -> Reply) -> Reply {
    state.api_calls += 1;
    if state.rate_limit_next > 0 {
        state.rate_limit_next -= 1;
        return Reply::empty(StatusCode::TOO_MANY_REQUESTS).with_header("retry-after", "0");
    }
    let token = request.header("authorization").and_then(|value| value.strip_prefix("Bearer "));
    if !token.is_some_and(|token| state.oauth.access_valid(token)) {
        return Reply::json(StatusCode::UNAUTHORIZED, &serde_json::json!({"error": "invalid_token"}));
    }
    handler(state, request)
}
