//! A tiny authorization server: authorization code + PKCE (S256), refresh
//! tokens (optionally rotating), expiring access tokens.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hyper::StatusCode;
use sha2::{Digest, Sha256};

use crate::State;
use crate::http::{Reply, Request};

#[derive(Debug)]
struct PendingCode {
    challenge: String,
    redirect_uri: String,
}

#[derive(Debug)]
pub struct OAuthState {
    codes: HashMap<String, PendingCode>,
    refresh_tokens: HashMap<String, ()>,
    access_tokens: HashMap<String, Instant>,
    next: u64,
    pub access_lifetime: Duration,
    /// Every refresh returns a new refresh token and retires the old one (Microsoft).
    pub rotate_refresh_tokens: bool,
    /// The user clicks "Deny".
    pub deny: bool,
    pub refreshes: u32,
    /// The scopes the last sign-in asked for.
    pub last_scope: String,
}

impl Default for OAuthState {
    fn default() -> Self {
        Self {
            codes: HashMap::new(),
            refresh_tokens: HashMap::new(),
            access_tokens: HashMap::new(),
            next: 0,
            access_lifetime: Duration::from_secs(3600),
            rotate_refresh_tokens: false,
            deny: false,
            refreshes: 0,
            last_scope: String::new(),
        }
    }
}

impl OAuthState {
    fn token(&mut self, kind: &str) -> String {
        self.next += 1;
        format!("{kind}-{}", self.next)
    }

    pub fn issue_refresh(&mut self) -> String {
        let token = self.token("refresh");
        self.refresh_tokens.insert(token.clone(), ());
        token
    }

    fn issue_access(&mut self) -> String {
        let token = self.token("access");
        self.access_tokens.insert(token.clone(), Instant::now() + self.access_lifetime);
        token
    }

    pub fn access_valid(&self, token: &str) -> bool {
        self.access_tokens.get(token).is_some_and(|until| Instant::now() < *until)
    }

    /// Every access token stops working now (as after an hour, or a revocation).
    pub fn expire_access_tokens(&mut self) {
        self.access_tokens.clear();
    }

    pub fn revoke_refresh_tokens(&mut self) {
        self.refresh_tokens.clear();
    }

    fn tokens(&mut self, refresh: Option<String>) -> serde_json::Value {
        let access = self.issue_access();
        let mut body = serde_json::json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": self.access_lifetime.as_secs(),
        });
        if let Some(refresh) = refresh {
            body["refresh_token"] = refresh.into();
        }
        body
    }
}

pub fn handle(state: &mut State, rest: &str, request: &Request) -> Reply {
    match (request.method.as_str(), rest) {
        ("GET", "authorize") => authorize(&mut state.oauth, request),
        ("POST", "token") => token(&mut state.oauth, request),
        _ => Reply::empty(StatusCode::NOT_FOUND),
    }
}

fn authorize(oauth: &mut OAuthState, request: &Request) -> Reply {
    let param = |key: &str| request.query.get(key).cloned().unwrap_or_default();
    let (redirect, state) = (param("redirect_uri"), param("state"));
    oauth.last_scope = param("scope");
    let bad =
        param("response_type") != "code" || param("code_challenge_method") != "S256" || param("client_id").is_empty();
    let location = if oauth.deny || bad {
        format!("{redirect}?error=access_denied&error_description=The+user+denied+access&state={state}")
    } else {
        let code = oauth.token("code");
        oauth
            .codes
            .insert(code.clone(), PendingCode { challenge: param("code_challenge"), redirect_uri: redirect.clone() });
        format!("{redirect}?code={code}&state={state}")
    };
    Reply::empty(StatusCode::FOUND).with_header("location", &location)
}

fn token(oauth: &mut OAuthState, request: &Request) -> Reply {
    let form = request.form();
    let field = |key: &str| form.get(key).cloned().unwrap_or_default();
    let invalid = |description: &str| {
        Reply::json(
            StatusCode::BAD_REQUEST,
            &serde_json::json!({"error": "invalid_grant", "error_description": description}),
        )
    };
    match field("grant_type").as_str() {
        "authorization_code" => {
            let Some(pending) = oauth.codes.remove(&field("code")) else { return invalid("unknown code") };
            let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(field("code_verifier").as_bytes()));
            if challenge != pending.challenge {
                return invalid("PKCE verification failed");
            }
            if pending.redirect_uri != field("redirect_uri") {
                return invalid("redirect_uri mismatch");
            }
            let refresh = oauth.issue_refresh();
            Reply::ok(&oauth.tokens(Some(refresh)))
        }
        "refresh_token" => {
            let refresh = field("refresh_token");
            if oauth.refresh_tokens.remove(&refresh).is_none() {
                return invalid("refresh token revoked or expired");
            }
            oauth.refreshes += 1;
            if oauth.rotate_refresh_tokens {
                let rotated = oauth.issue_refresh();
                Reply::ok(&oauth.tokens(Some(rotated)))
            } else {
                oauth.refresh_tokens.insert(refresh, ());
                Reply::ok(&oauth.tokens(None))
            }
        }
        _ => invalid("unsupported grant_type"),
    }
}
