//! Doubles of the cloud model APIs (Anthropic Messages, OpenAI-compatible
//! chat completions): every call is recorded, and answered with the next
//! scripted reply.

use hyper::StatusCode;

use crate::State;
use crate::http::{Reply, Request};

/// One recorded call to a model API.
#[derive(Debug, Clone)]
pub struct ModelCall {
    pub path: String,
    /// Lower-case header names.
    pub headers: std::collections::HashMap<String, String>,
    pub body: serde_json::Value,
}

pub(crate) fn handle(state: &mut State, request: &Request) -> Reply {
    state.model_calls.push(ModelCall {
        path: request.path.clone(),
        headers: request.headers.clone(),
        body: request.json(),
    });
    match state.model_replies.pop_front() {
        Some((status, body)) => Reply::json(StatusCode::from_u16(status).unwrap_or(StatusCode::OK), &body),
        None => Reply::json(StatusCode::INTERNAL_SERVER_ERROR, &serde_json::json!({"error": "no reply scripted"})),
    }
}
