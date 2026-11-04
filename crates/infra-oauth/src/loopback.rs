//! The redirect target: a one-shot HTTP listener on the loopback interface
//! (RFC 8252 §7.3). It answers the browser with a short page and hands the
//! authorization code to the waiting sign-in.

use application::ports::SignInError;
use domain::VfsError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_REQUEST: usize = 16 * 1024;
const HEADER_END: &[u8] = b"\r\n\r\n";
const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>FinderMaxxing</title>\
<body style=\"font-family:system-ui;margin:4em\"><h1>You're signed in</h1>\
<p>You can close this tab and return to FinderMaxxing.</p>";
const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>FinderMaxxing</title>\
<body style=\"font-family:system-ui;margin:4em\"><h1>Signing in didn't work</h1>\
<p>Return to FinderMaxxing to try again.</p>";

/// Where providers send the browser back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirect {
    /// `http://127.0.0.1:<any free port>/` (Google, Microsoft).
    AnyPort { host: &'static str },
    /// A port registered with the provider (Dropbox wants exact URIs).
    FixedPort { host: &'static str, port: u16 },
}

pub(crate) struct Listener {
    listener: TcpListener,
    host: &'static str,
}

impl Listener {
    pub(crate) async fn bind(redirect: &Redirect) -> Result<Self, VfsError> {
        let (host, port) = match redirect {
            Redirect::AnyPort { host } => (*host, 0),
            Redirect::FixedPort { host, port } => (*host, *port),
        };
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| VfsError::Io { message: format!("couldn't listen for the sign-in: {e}") })?;
        Ok(Self { listener, host })
    }

    pub(crate) fn redirect_uri(&self) -> String {
        let port = self.listener.local_addr().map(|a| a.port()).unwrap_or_default();
        format!("http://{}:{port}/", self.host)
    }

    /// Waits for the browser to arrive with `code` (and our `state`).
    /// Unrelated requests (favicons) are answered and ignored.
    pub(crate) async fn code(&self, expected_state: &str) -> Result<String, SignInError> {
        loop {
            let (mut stream, _) = self.listener.accept().await.map_err(|e| VfsError::Io { message: e.to_string() })?;
            let Some(query) = read_query(&mut stream).await else {
                respond(&mut stream, "404 Not Found", "").await;
                continue;
            };
            let params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
            let value = |key: &str| params.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            let outcome = match (value("code"), value("error"), value("state")) {
                (_, Some(error), _) => Err(SignInError::Denied(value("error_description").unwrap_or(error))),
                (Some(code), None, Some(state)) if state == expected_state => Ok(code),
                (Some(_), None, _) => Err(SignInError::Denied("the sign-in answer didn't match this attempt".into())),
                (None, None, _) => {
                    respond(&mut stream, "404 Not Found", "").await;
                    continue;
                }
            };
            let page = if outcome.is_ok() { DONE_PAGE } else { FAILED_PAGE };
            respond(&mut stream, "200 OK", page).await;
            return outcome;
        }
    }
}

/// The query of a `GET /?…` request line, if that's what arrived.
async fn read_query(stream: &mut TcpStream) -> Option<String> {
    let mut request = Vec::new();
    let mut buffer = [0; 2048];
    while !request.windows(HEADER_END.len()).any(|w| w == HEADER_END) && request.len() < MAX_REQUEST {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let line = std::str::from_utf8(&request).ok()?.lines().next()?.to_owned();
    let target = line.strip_prefix("GET ")?.split(' ').next()?;
    let (path, query) = target.split_once('?')?;
    (path == "/").then(|| query.to_owned())
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.shutdown().await;
}
