//! The OAuth flow and authenticated requests against the mock authorization server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::{Arc, Mutex};

use application::ports::{Secret, SignInError, UrlOpener};
use application::{CancelToken, CancellationSource};
use domain::VfsError;
use futures::FutureExt;
use futures::future::BoxFuture;
use infra_oauth::{ApiClient, OAuthApp, Redirect, TokenSource, authorize};
use mock_cloud::MockCloud;

/// A browser whose user clicks "Allow" (the mock decides what happens).
struct Browser;

impl UrlOpener for Browser {
    fn open_url(&self, url: &str) -> BoxFuture<'static, Result<(), VfsError>> {
        let url = url.to_owned();
        async move {
            // The app awaits the redirect concurrently; visit in the background.
            tokio::spawn(async move { MockCloud::visit_as_browser(&url).await });
            Ok(())
        }
        .boxed()
    }
}

/// A browser window the user never comes back from.
struct IgnoredBrowser;

impl UrlOpener for IgnoredBrowser {
    fn open_url(&self, _url: &str) -> BoxFuture<'static, Result<(), VfsError>> {
        futures::future::ready(Ok(())).boxed()
    }
}

fn app(cloud: &MockCloud) -> OAuthApp {
    OAuthApp {
        client_id: "test-client".into(),
        client_secret: None,
        authorize_url: format!("{}/oauth/authorize", cloud.base()),
        token_url: format!("{}/oauth/token", cloud.base()),
        scopes: vec!["files".into(), "offline".into()],
        extra_authorize_params: vec![("access_type".into(), "offline".into())],
        redirect: Redirect::AnyPort { host: "127.0.0.1" },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_exchanges_a_pkce_bound_code_for_tokens() {
    let cloud = MockCloud::start().await;
    let http = reqwest::Client::new();
    let tokens = authorize(&app(&cloud), &http, &Browser, CancelToken::never()).await.unwrap();
    assert!(tokens.refresh_token.is_some());
    assert_eq!(cloud.state().oauth.last_scope, "files offline");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_consent_is_reported_and_an_abandoned_one_can_be_cancelled() {
    let cloud = MockCloud::start().await;
    cloud.state().oauth.deny = true;
    let http = reqwest::Client::new();
    let denied = authorize(&app(&cloud), &http, &Browser, CancelToken::never()).await;
    assert_eq!(denied, Err(SignInError::Denied("The user denied access".into())));

    let source = CancellationSource::new();
    let token = source.token();
    let waiting = tokio::spawn({
        let app = app(&cloud);
        async move { authorize(&app, &reqwest::Client::new(), &IgnoredBrowser, token).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    source.cancel();
    assert_eq!(waiting.await.unwrap(), Err(SignInError::Cancelled));
}

fn client(cloud: &MockCloud, refresh: &str, rotated: Arc<Mutex<Vec<String>>>) -> ApiClient {
    let http = reqwest::Client::new();
    let on_rotated = Arc::new(move |token: Secret| rotated.lock().unwrap().push(token.expose().to_owned()));
    let tokens = TokenSource::new(app(cloud), http.clone(), Secret::new(refresh.into()), on_rotated);
    ApiClient::new(http, Arc::new(tokens))
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_access_tokens_are_refreshed_and_rotations_reported() {
    let cloud = MockCloud::start().await;
    cloud.state().oauth.rotate_refresh_tokens = true;
    let refresh = cloud.issue_refresh_token();
    let rotated = Arc::new(Mutex::new(Vec::new()));
    let api = client(&cloud, &refresh, rotated.clone());
    let about = format!("{}/gdrive/drive/v3/about", cloud.base());

    assert!(api.send(|http| http.get(&about)).await.unwrap().status().is_success());
    cloud.state().oauth.expire_access_tokens();
    assert!(api.send(|http| http.get(&about)).await.unwrap().status().is_success(), "refreshed after the 401");
    assert_eq!(cloud.state().oauth.refreshes, 2);
    assert_eq!(rotated.lock().unwrap().len(), 2, "each rotation is handed over for saving");
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limits_are_waited_out_and_revoked_grants_need_a_new_sign_in() {
    let cloud = MockCloud::start().await;
    let refresh = cloud.issue_refresh_token();
    let api = client(&cloud, &refresh, Arc::default());
    let about = format!("{}/gdrive/drive/v3/about", cloud.base());

    cloud.state().rate_limit_next = 2;
    assert!(api.send(|http| http.get(&about)).await.unwrap().status().is_success());

    cloud.state().oauth.expire_access_tokens();
    cloud.state().oauth.revoke_refresh_tokens();
    assert_eq!(api.send(|http| http.get(&about)).await.map(|r| r.status()), Err(VfsError::PermissionDenied));
}
