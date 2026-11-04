//! `gdrive://` provider: Google Drive API v3 (ADR 0024).
//!
//! Roles: `Lister`, `Reader` (ranged; Google Docs exported to Office
//! formats), `Writer` (resumable upload sessions), `Copier` (server-side),
//! `Mover`, `Remover`, `FolderMaker` and `Trasher` (Drive's own Trash).
//! Drive allows duplicate names in a folder; the app shows them as
//! `report (2).pdf` and never renames anything on Drive.

mod fs;
mod listing;

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use application::CancelToken;
use application::ports::{
    CredentialKey, CredentialStore, DriveConnector, MountHandle, ProviderFactory, Secret, SignInError, SignedIn,
    UrlOpener,
};
use domain::{
    AccountId, Capabilities, Capability, DriveAccount, DriveLocation, DriveProvider, MountId, PathError, Scheme,
    StoragePath, VfsError,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use infra_oauth::{ApiClient, OAuthApp, Redirect, account_client, authorize, network_error};
use tokio::runtime::Handle;

pub use fs::DriveFs;

const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";
const LOOPBACK_HOST: &str = "127.0.0.1";

const DRIVE_SEMANTICS: Capabilities = Capabilities::empty()
    .with(Capability::AtomicRename)
    .with(Capability::ServerSideCopy)
    .with(Capability::ResumableUpload);

/// Where Google lives; tests point these at a mock.
#[derive(Debug, Clone)]
pub struct GoogleDriveEndpoints {
    pub api: String,
    pub authorize: String,
    pub token: String,
}

impl Default for GoogleDriveEndpoints {
    fn default() -> Self {
        Self {
            api: "https://www.googleapis.com".into(),
            authorize: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
        }
    }
}

/// This build's OAuth client ("Desktop app" type in the Google Cloud console).
#[derive(Debug, Clone, Default)]
pub struct GoogleDriveSettings {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub endpoints: GoogleDriveEndpoints,
}

struct Inner {
    scheme: Scheme,
    runtime: Handle,
    settings: GoogleDriveSettings,
    http: reqwest::Client,
    opener: Arc<dyn UrlOpener>,
    credentials: Arc<dyn CredentialStore>,
    accounts: RwLock<HashMap<AccountId, ApiClient>>,
}

/// Connected accounts and the `gdrive://` provider factory. Cheap to clone.
#[derive(Clone)]
pub struct GoogleDriveService {
    inner: Arc<Inner>,
}

impl GoogleDriveService {
    /// # Errors
    /// Never in practice; the scheme literal is valid.
    pub fn new(
        runtime: Handle,
        settings: GoogleDriveSettings,
        opener: Arc<dyn UrlOpener>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, PathError> {
        let inner = Inner {
            scheme: DriveProvider::GoogleDrive.scheme()?,
            runtime,
            settings,
            http: reqwest::Client::new(),
            opener,
            credentials,
            accounts: RwLock::default(),
        };
        Ok(Self { inner: Arc::new(inner) })
    }

    fn app(&self) -> Option<OAuthApp> {
        let settings = &self.inner.settings;
        Some(OAuthApp {
            client_id: settings.client_id.clone()?,
            client_secret: settings.client_secret.clone(),
            authorize_url: settings.endpoints.authorize.clone(),
            token_url: settings.endpoints.token.clone(),
            scopes: vec![DRIVE_SCOPE.into()],
            // A refresh token, every time (Google only sends one on consent).
            extra_authorize_params: vec![("access_type".into(), "offline".into()), ("prompt".into(), "consent".into())],
            redirect: Redirect::AnyPort { host: LOOPBACK_HOST },
        })
    }

    /// The account's roles, for callers that need the concrete type (tests).
    ///
    /// # Errors
    /// `Unsupported(List)` if the account isn't connected.
    pub fn open_account(&self, id: AccountId) -> Result<DriveFs, VfsError> {
        let accounts = self.inner.accounts.read().unwrap_or_else(PoisonError::into_inner);
        let api = accounts.get(&id).cloned().ok_or(VfsError::Unsupported(Capability::List))?;
        let mount = DriveLocation { provider: DriveProvider::GoogleDrive, account: id }.mount()?;
        Ok(DriveFs::new(
            api,
            self.inner.settings.endpoints.api.clone(),
            StoragePath::root(mount),
            self.inner.runtime.clone(),
        ))
    }
}

impl std::fmt::Debug for GoogleDriveService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleDriveService").finish_non_exhaustive()
    }
}

/// The signed-in user, from `about`.
async fn whoami(api: &ApiClient, base: &str) -> Result<(String, String), VfsError> {
    let url = format!("{base}/drive/v3/about");
    let response = api.send(|http| http.get(&url).query(&[("fields", "user")])).await?;
    if !response.status().is_success() {
        return Err(if response.status().as_u16() == 401 {
            VfsError::PermissionDenied
        } else {
            VfsError::Io { message: format!("Google Drive answered {}", response.status()) }
        });
    }
    let about: serde_json::Value = response.json().await.map_err(|e| network_error(&e))?;
    let user = &about["user"];
    Ok((
        user["emailAddress"].as_str().unwrap_or_default().to_owned(),
        user["displayName"].as_str().unwrap_or_default().to_owned(),
    ))
}

impl GoogleDriveService {
    fn client(&self, app: OAuthApp, refresh: Secret, id: Option<AccountId>) -> ApiClient {
        let inner = &self.inner;
        account_client(
            app,
            &inner.http,
            refresh,
            &inner.credentials,
            id.map(CredentialKey::drive_token),
            &inner.runtime,
        )
    }
}

impl DriveConnector for GoogleDriveService {
    fn provider(&self) -> DriveProvider {
        DriveProvider::GoogleDrive
    }

    fn sign_in(&self, cancel: CancelToken) -> BoxFuture<'static, Result<SignedIn, SignInError>> {
        let service = self.clone();
        async move {
            let app = service.app().ok_or(SignInError::NotConfigured(DriveProvider::GoogleDrive))?;
            let tokens = authorize(&app, &service.inner.http, &*service.inner.opener, cancel).await?;
            let refresh = tokens.refresh_token.ok_or(SignInError::Denied("Google sent no refresh token".into()))?;
            let api = service.client(app, refresh.clone(), None);
            let (email, display_name) = whoami(&api, &service.inner.settings.endpoints.api).await?;
            Ok(SignedIn { email, display_name, refresh_token: refresh })
        }
        .boxed()
    }

    fn connect(&self, account: &DriveAccount, refresh_token: Secret) -> BoxFuture<'static, Result<(), VfsError>> {
        let (service, id) = (self.clone(), account.id);
        async move {
            let app = service.app().ok_or(VfsError::Unsupported(Capability::List))?;
            let api = service.client(app, refresh_token, Some(id));
            whoami(&api, &service.inner.settings.endpoints.api).await?;
            service.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).insert(id, api);
            Ok(())
        }
        .boxed()
    }

    fn disconnect(&self, id: AccountId) {
        self.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }
}

impl ProviderFactory for GoogleDriveService {
    fn scheme(&self) -> Scheme {
        self.inner.scheme.clone()
    }

    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        let location = DriveLocation::of(mount)
            .ok_or_else(|| VfsError::InvalidPath(PathError::InvalidAuthority(mount.authority().into())))?;
        let fs = Arc::new(self.open_account(location.account)?);
        Ok(MountHandle::builder(DRIVE_SEMANTICS)
            .lister(fs.clone())
            .reader(fs.clone())
            .writer(fs.clone())
            .copier(fs.clone())
            .mover(fs.clone())
            .remover(fs.clone())
            .folder_maker(fs.clone())
            .trasher(fs)
            .build())
    }
}
