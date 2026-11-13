//! One local HTTP server standing in for the cloud: an OAuth authorization
//! server and the parts of the Google Drive v3, Dropbox v2 and Microsoft
//! Graph APIs the adapters use, each over its own in-memory tree.
//!
//! The doubles follow the providers' documented behaviour, including the
//! awkward parts (Drive's duplicate names and 256 KiB upload chunks,
//! Dropbox's `incorrect_offset`, Graph's pre-authorised upload URLs), so the
//! adapters' contract tests mean something before real accounts exist.

#![allow(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::expect_used,
    reason = "test double: a broken setup should fail the test loudly"
)]

mod dropbox;
mod google;
mod graph;
mod http;
pub mod models;
mod oauth;
mod tree;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

pub use tree::{Item, Tree};

/// What the server knows; tests reach in through [`MockCloud`] to seed
/// data or bend the rules (expire tokens, deny sign-in, rate-limit).
#[derive(Debug)]
pub struct State {
    pub base: String,
    pub google: Tree,
    pub dropbox: Tree,
    pub onedrive: Tree,
    pub oauth: oauth::OAuthState,
    pub uploads: HashMap<String, Upload>,
    pub next_upload: u64,
    /// The next N API calls answer 429 with `Retry-After: 0`.
    pub rate_limit_next: u32,
    pub api_calls: u64,
    /// Calls to the model APIs, in order.
    pub model_calls: Vec<models::ModelCall>,
    /// What the model APIs answer next: status and JSON body.
    pub model_replies: std::collections::VecDeque<(u16, serde_json::Value)>,
}

/// A resumable upload in progress (Drive session, Dropbox session, Graph session).
#[derive(Debug, Clone)]
pub struct Upload {
    pub parent: String,
    pub name: String,
    pub received: Vec<u8>,
    pub total: Option<u64>,
    /// Dropbox: no more appends (`close: true`).
    pub closed: bool,
}

/// A running mock; dropped servers keep running until the test ends.
#[derive(Clone)]
pub struct MockCloud {
    state: Arc<Mutex<State>>,
    base: String,
}

impl std::fmt::Debug for MockCloud {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockCloud").field("base", &self.base).finish_non_exhaustive()
    }
}

impl MockCloud {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let base = format!("http://{}", listener.local_addr().expect("bound"));
        let state = Arc::new(Mutex::new(State {
            base: base.clone(),
            google: Tree::new(),
            dropbox: Tree::new(),
            onedrive: Tree::new(),
            oauth: oauth::OAuthState::default(),
            uploads: HashMap::new(),
            next_upload: 0,
            rate_limit_next: 0,
            api_calls: 0,
            model_calls: Vec::new(),
            model_replies: std::collections::VecDeque::new(),
        }));
        let shared = Arc::clone(&state);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = Arc::clone(&shared);
                tokio::spawn(async move {
                    let service = service_fn(move |request| http::handle(Arc::clone(&state), request));
                    let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
                });
            }
        });
        Self { state, base }
    }

    /// `http://127.0.0.1:<port>`.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A refresh token as if the user had signed in long ago.
    #[must_use]
    pub fn issue_refresh_token(&self) -> String {
        self.state().oauth.issue_refresh()
    }

    /// Seeds a Google Doc (`kind` = `document`, `spreadsheet`, `presentation`)
    /// in the Drive folder with id `parent`.
    pub fn add_google_doc(&self, parent: &str, name: &str, kind: &str) -> String {
        google::add_google_doc(&mut self.state().google, parent, name, kind)
    }

    /// Plays the browser: follows the authorize URL's redirect to the app's
    /// loopback listener, like a user who clicks "Allow".
    pub async fn visit_as_browser(url: &str) -> Result<(), String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let answer = client.get(url).send().await.map_err(|e| e.to_string())?;
        let location = answer.headers().get("location").and_then(|v| v.to_str().ok()).ok_or("no redirect")?.to_owned();
        client.get(location).send().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}
