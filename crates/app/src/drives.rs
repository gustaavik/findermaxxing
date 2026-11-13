//! Cloud-drive providers and their OAuth clients (docs/oauth-setup.md).
//!
//! Client IDs are compiled in from `FM_GOOGLE_CLIENT_ID`,
//! `FM_GOOGLE_CLIENT_SECRET`, `FM_DROPBOX_APP_KEY` and
//! `FM_ONEDRIVE_CLIENT_ID`. A provider without one says so when the user
//! tries to add it. Debug builds also read them at run time, and
//! `FM_DEV_CLOUD_MOCK=<url of mock-cloud-server>` points every provider at
//! the local API doubles with a headless "browser": for development only;
//! release builds never honour it.

use std::sync::Arc;

use adapter_dropbox::{DropboxEndpoints, DropboxService, DropboxSettings};
use adapter_gdrive::{GoogleDriveEndpoints, GoogleDriveService, GoogleDriveSettings};
use adapter_onedrive::{OneDriveEndpoints, OneDriveService, OneDriveSettings};
use application::ports::{CredentialStore, DriveConnector, UrlOpener};
use domain::{PathError, VfsError};
use futures::FutureExt;
use futures::future::BoxFuture;
use infra_oauth::Redirect;
use tokio::runtime::Handle;

const MOCK_VARIABLE: &str = "FM_DEV_CLOUD_MOCK";
const MOCK_CLIENT_ID: &str = "dev-mock";

/// A compiled-in setting; in debug builds the environment may override it.
pub(crate) fn client_setting(compiled: Option<&'static str>, variable: &str) -> Option<String> {
    let runtime = cfg!(debug_assertions).then(|| std::env::var(variable).ok()).flatten();
    runtime.or_else(|| compiled.map(str::to_owned)).filter(|value| !value.trim().is_empty())
}

fn mock_base() -> Option<String> {
    cfg!(debug_assertions).then(|| std::env::var(MOCK_VARIABLE).ok()).flatten()
}

pub(crate) struct Drives {
    pub google: GoogleDriveService,
    pub dropbox: DropboxService,
    pub onedrive: OneDriveService,
}

impl Drives {
    pub(crate) fn new(
        runtime: &Handle,
        browser: Arc<dyn UrlOpener>,
        credentials: &Arc<dyn CredentialStore>,
    ) -> Result<Self, PathError> {
        let mock = mock_base();
        let browser: Arc<dyn UrlOpener> = if mock.is_some() { Arc::new(HeadlessBrowser) } else { browser };
        let (mut google, mut dropbox, mut onedrive) = (
            GoogleDriveSettings {
                client_id: client_setting(option_env!("FM_GOOGLE_CLIENT_ID"), "FM_GOOGLE_CLIENT_ID"),
                client_secret: client_setting(option_env!("FM_GOOGLE_CLIENT_SECRET"), "FM_GOOGLE_CLIENT_SECRET"),
                endpoints: GoogleDriveEndpoints::default(),
            },
            DropboxSettings {
                client_id: client_setting(option_env!("FM_DROPBOX_APP_KEY"), "FM_DROPBOX_APP_KEY"),
                ..DropboxSettings::default()
            },
            OneDriveSettings {
                client_id: client_setting(option_env!("FM_ONEDRIVE_CLIENT_ID"), "FM_ONEDRIVE_CLIENT_ID"),
                ..OneDriveSettings::default()
            },
        );
        if let Some(base) = mock {
            let (authorize, token) = (format!("{base}/oauth/authorize"), format!("{base}/oauth/token"));
            google.client_id = Some(MOCK_CLIENT_ID.into());
            google.endpoints = GoogleDriveEndpoints {
                api: format!("{base}/gdrive"),
                authorize: authorize.clone(),
                token: token.clone(),
            };
            dropbox.client_id = Some(MOCK_CLIENT_ID.into());
            dropbox.redirect = Redirect::AnyPort { host: "127.0.0.1" };
            dropbox.endpoints = DropboxEndpoints {
                rpc: format!("{base}/dropbox/api"),
                content: format!("{base}/dropbox/content"),
                authorize: authorize.clone(),
                token: token.clone(),
            };
            onedrive.client_id = Some(MOCK_CLIENT_ID.into());
            onedrive.endpoints = OneDriveEndpoints { graph: format!("{base}/graph/v1.0"), authorize, token };
        }
        Ok(Self {
            google: GoogleDriveService::new(runtime.clone(), google, browser.clone(), credentials.clone())?,
            dropbox: DropboxService::new(runtime.clone(), dropbox, browser.clone(), credentials.clone())?,
            onedrive: OneDriveService::new(runtime.clone(), onedrive, browser, credentials.clone())?,
        })
    }

    pub(crate) fn connectors(&self) -> Vec<Arc<dyn DriveConnector>> {
        vec![Arc::new(self.google.clone()), Arc::new(self.dropbox.clone()), Arc::new(self.onedrive.clone())]
    }
}

/// Development only: "clicks Allow" on the mock's sign-in page by following
/// its redirect to the app's loopback listener.
struct HeadlessBrowser;

impl UrlOpener for HeadlessBrowser {
    fn open_url(&self, url: &str) -> BoxFuture<'static, Result<(), VfsError>> {
        let url = url.to_owned();
        tokio::spawn(async move {
            let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().ok()?;
            let answer = client.get(&url).send().await.ok()?;
            let location = answer.headers().get("location")?.to_str().ok()?.to_owned();
            client.get(location).send().await.ok()
        });
        futures::future::ready(Ok(())).boxed()
    }
}
