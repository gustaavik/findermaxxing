//! OAuth 2.0 for native apps (RFC 8252) and authenticated HTTP for the
//! cloud-drive adapters (ADR 0023).
//!
//! - [`authorize`]: the browser sign-in. A one-shot HTTP listener on the
//!   loopback interface receives the redirect; PKCE (S256) binds the code to
//!   this process, and `state` to this attempt.
//! - [`TokenSource`]: hands out access tokens, refreshing them before they
//!   expire (or when the provider rejects one), and reports rotated refresh
//!   tokens so they can be saved.
//! - [`ApiClient`]: sends requests with the bearer token, retrying once after
//!   a refresh and backing off when the provider is rate-limiting.

mod loopback;
mod pkce;
mod time;

use std::sync::Arc;
use std::time::{Duration, Instant};

use application::CancelToken;
use application::ports::{CredentialKey, CredentialStore, Secret, SignInError, UrlOpener};
use domain::VfsError;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::Deserialize;

pub use loopback::Redirect;
pub use time::parse_rfc3339;

/// Give the user this long to finish signing in.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Refresh this long before the provider's stated expiry.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);
/// Assumed lifetime when the provider doesn't say.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(30 * 60);
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);
const DEFAULT_RETRY_WAIT: Duration = Duration::from_secs(2);
const RETRY_AFTER: &str = "Retry-After";

/// A provider's OAuth endpoints and this app's registration with it.
#[derive(Debug, Clone)]
pub struct OAuthApp {
    pub client_id: String,
    /// Only Google's desktop clients have one; it is not a secret in a
    /// distributed app (RFC 8252 §8.5), PKCE is what protects the flow.
    pub client_secret: Option<String>,
    pub authorize_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    /// Provider-specific authorize parameters (`access_type=offline`, …).
    pub extra_authorize_params: Vec<(String, String)>,
    pub redirect: Redirect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: Secret,
    /// Absent when refreshing with a provider that doesn't rotate.
    pub refresh_token: Option<Secret>,
    pub expires_in: Option<Duration>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Runs the browser sign-in and exchanges the code for tokens.
///
/// # Errors
/// `Cancelled` (by `cancel` or after ten minutes), `Denied` when the user or
/// provider refused, `Failed` for network and protocol problems.
pub async fn authorize(
    app: &OAuthApp,
    http: &reqwest::Client,
    opener: &dyn UrlOpener,
    cancel: CancelToken,
) -> Result<Tokens, SignInError> {
    let listener = loopback::Listener::bind(&app.redirect).await?;
    let redirect_uri = listener.redirect_uri();
    let (verifier, challenge) = pkce::verifier_and_challenge()?;
    let state = pkce::random_token()?;
    let mut url = url::Url::parse(&app.authorize_url).map_err(|e| io_error(&e))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &app.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &app.scopes.join(" "))
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .extend_pairs(app.extra_authorize_params.iter());
    opener.open_url(url.as_str()).await?;
    let code = tokio::select! {
        code = listener.code(&state) => code?,
        () = cancel.cancelled() => return Err(SignInError::Cancelled),
        () = tokio::time::sleep(SIGN_IN_TIMEOUT) => return Err(SignInError::Cancelled),
    };
    let mut form = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", app.client_id.clone()),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = &app.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    token_request(http, &app.token_url, &form).await
}

/// Trades a refresh token for a new access token.
///
/// # Errors
/// `PermissionDenied` when the grant was revoked or expired (the user must
/// sign in again); `Transient`/`Io` for network problems.
pub async fn refresh(app: &OAuthApp, http: &reqwest::Client, refresh_token: &Secret) -> Result<Tokens, VfsError> {
    let mut form = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh_token.expose().to_owned()),
        ("client_id", app.client_id.clone()),
    ];
    if let Some(secret) = &app.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    token_request(http, &app.token_url, &form).await.map_err(|error| match error {
        SignInError::Denied(_) => VfsError::PermissionDenied,
        SignInError::Failed(error) => error,
        SignInError::Cancelled | SignInError::NotConfigured(_) => VfsError::Cancelled,
    })
}

async fn token_request(
    http: &reqwest::Client,
    token_url: &str,
    form: &[(&str, String)],
) -> Result<Tokens, SignInError> {
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(form.iter()).finish();
    let response = http
        .post(token_url)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| network_error(&e))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| network_error(&e))?;
    let parsed: TokenResponse = serde_json::from_str(&text)
        .map_err(|_| VfsError::Io { message: format!("the sign-in server answered {status}") })?;
    if let Some(error) = parsed.error {
        return Err(SignInError::Denied(parsed.error_description.unwrap_or(error)));
    }
    let access_token = parsed.access_token.ok_or_else(|| VfsError::Io { message: "no access token".into() })?;
    Ok(Tokens {
        access_token: Secret::new(access_token),
        refresh_token: parsed.refresh_token.map(Secret::new),
        expires_in: parsed.expires_in.map(Duration::from_secs),
    })
}

/// Called with a new refresh token when the provider rotates it.
pub type RotationCallback = Arc<dyn Fn(Secret) + Send + Sync>;

struct Current {
    access: Option<(Secret, Instant)>,
    refresh: Secret,
}

/// Access tokens for one account, refreshed on demand.
pub struct TokenSource {
    app: OAuthApp,
    http: reqwest::Client,
    current: tokio::sync::Mutex<Current>,
    on_rotated: RotationCallback,
}

impl TokenSource {
    #[must_use]
    pub fn new(app: OAuthApp, http: reqwest::Client, refresh_token: Secret, on_rotated: RotationCallback) -> Self {
        Self {
            app,
            http,
            current: tokio::sync::Mutex::new(Current { access: None, refresh: refresh_token }),
            on_rotated,
        }
    }

    /// A valid access token, refreshing first if needed.
    ///
    /// # Errors
    /// As [`refresh`].
    pub async fn access_token(&self) -> Result<Secret, VfsError> {
        let mut current = self.current.lock().await;
        if let Some((token, until)) = &current.access
            && Instant::now() < *until
        {
            return Ok(token.clone());
        }
        let tokens = refresh(&self.app, &self.http, &current.refresh).await?;
        let lifetime = tokens.expires_in.unwrap_or(DEFAULT_LIFETIME).saturating_sub(EXPIRY_MARGIN);
        current.access = Some((tokens.access_token.clone(), Instant::now() + lifetime));
        if let Some(rotated) = tokens.refresh_token
            && rotated != current.refresh
        {
            current.refresh = rotated.clone();
            (self.on_rotated)(rotated);
        }
        Ok(tokens.access_token)
    }

    /// Forgets the access token, e.g. after the provider rejected it.
    pub async fn invalidate(&self) {
        self.current.lock().await.access = None;
    }
}

impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSource").field("token_url", &self.app.token_url).finish_non_exhaustive()
    }
}

/// HTTP with the account's bearer token.
#[derive(Clone)]
pub struct ApiClient {
    pub http: reqwest::Client,
    tokens: Arc<TokenSource>,
}

impl ApiClient {
    #[must_use]
    pub fn new(http: reqwest::Client, tokens: Arc<TokenSource>) -> Self {
        Self { http, tokens }
    }

    /// Sends the request `build` makes. It is rebuilt for retries: once with
    /// a fresh token after a 401, and after waiting on 429/503.
    ///
    /// # Errors
    /// Network failures (`Transient`/`Io`) and token problems; HTTP error
    /// statuses are returned as responses for the adapter to interpret.
    pub async fn send(&self, build: impl Fn(&reqwest::Client) -> RequestBuilder) -> Result<Response, VfsError> {
        let mut refreshed = false;
        let mut waits = 0;
        loop {
            let token = self.tokens.access_token().await?;
            let response = build(&self.http).bearer_auth(token.expose()).send().await.map_err(|e| network_error(&e))?;
            match response.status() {
                StatusCode::UNAUTHORIZED if !refreshed => {
                    refreshed = true;
                    self.tokens.invalidate().await;
                }
                StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE if waits < MAX_RATE_LIMIT_RETRIES => {
                    waits += 1;
                    tokio::time::sleep(retry_wait(&response)).await;
                }
                _ => return Ok(response),
            }
        }
    }
}

/// An [`ApiClient`] for an account whose rotated refresh tokens are saved
/// under `key` (`None` while signing in, before the account exists).
#[must_use]
pub fn account_client(
    app: OAuthApp,
    http: &reqwest::Client,
    refresh: Secret,
    credentials: &Arc<dyn CredentialStore>,
    key: Option<CredentialKey>,
    runtime: &tokio::runtime::Handle,
) -> ApiClient {
    let (credentials, runtime) = (Arc::clone(credentials), runtime.clone());
    let on_rotated: RotationCallback = Arc::new(move |rotated: Secret| {
        if let Some(key) = key.clone() {
            let credentials = Arc::clone(&credentials);
            // Losing a rotation means signing in again; nothing else breaks.
            runtime.spawn(async move { credentials.store(&key, &rotated).await });
        }
    });
    let tokens = TokenSource::new(app, http.clone(), refresh, on_rotated);
    ApiClient::new(http.clone(), Arc::new(tokens))
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient").finish_non_exhaustive()
    }
}

fn retry_wait(response: &Response) -> Duration {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok()?.trim().parse::<u64>().ok())
        .map_or(DEFAULT_RETRY_WAIT, Duration::from_secs)
        .min(MAX_RETRY_WAIT)
}

#[must_use]
pub fn network_error(error: &reqwest::Error) -> VfsError {
    if error.is_timeout() || error.is_connect() {
        VfsError::Transient { retry_after: None }
    } else {
        VfsError::Io { message: error.to_string() }
    }
}

fn io_error(error: &dyn std::fmt::Display) -> VfsError {
    VfsError::Io { message: error.to_string() }
}
