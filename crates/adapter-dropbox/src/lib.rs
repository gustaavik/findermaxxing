//! `dropbox://` provider: Dropbox API v2 (ADR 0024).
//!
//! Roles: `Lister`, `Reader` (ranged), `Writer` (`upload`, or a resumable
//! upload session for large files, both `mode: add`), `Copier` and `Mover`
//! (server-side), `Remover`, `FolderMaker`. Dropbox paths are
//! case-insensitive; a case-only rename names the same item.

mod fs;

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

pub use fs::DropboxFs;

/// Dropbox wants exact redirect URIs; register `http://localhost:53682/`.
pub const REDIRECT_PORT: u16 = 53682;
const SCOPES: &[&str] = &["account_info.read", "files.metadata.read", "files.content.read", "files.content.write"];

const DROPBOX_SEMANTICS: Capabilities = Capabilities::empty()
    .with(Capability::AtomicRename)
    .with(Capability::ServerSideCopy)
    .with(Capability::ResumableUpload);

#[derive(Debug, Clone)]
pub struct DropboxEndpoints {
    pub rpc: String,
    pub content: String,
    pub authorize: String,
    pub token: String,
}

impl Default for DropboxEndpoints {
    fn default() -> Self {
        Self {
            rpc: "https://api.dropboxapi.com".into(),
            content: "https://content.dropboxapi.com".into(),
            authorize: "https://www.dropbox.com/oauth2/authorize".into(),
            token: "https://api.dropboxapi.com/oauth2/token".into(),
        }
    }
}

/// This build's app (the "App key"; PKCE, so no secret).
#[derive(Debug, Clone)]
pub struct DropboxSettings {
    pub client_id: Option<String>,
    pub endpoints: DropboxEndpoints,
    pub redirect: Redirect,
}

impl Default for DropboxSettings {
    fn default() -> Self {
        Self {
            client_id: None,
            endpoints: DropboxEndpoints::default(),
            redirect: Redirect::FixedPort { host: "localhost", port: REDIRECT_PORT },
        }
    }
}

struct Inner {
    scheme: Scheme,
    runtime: Handle,
    settings: DropboxSettings,
    http: reqwest::Client,
    opener: Arc<dyn UrlOpener>,
    credentials: Arc<dyn CredentialStore>,
    accounts: RwLock<HashMap<AccountId, ApiClient>>,
}

/// Connected accounts and the `dropbox://` provider factory. Cheap to clone.
#[derive(Clone)]
pub struct DropboxService {
    inner: Arc<Inner>,
}

impl DropboxService {
    /// # Errors
    /// Never in practice; the scheme literal is valid.
    pub fn new(
        runtime: Handle,
        settings: DropboxSettings,
        opener: Arc<dyn UrlOpener>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, PathError> {
        let inner = Inner {
            scheme: DriveProvider::Dropbox.scheme()?,
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
            client_secret: None,
            authorize_url: settings.endpoints.authorize.clone(),
            token_url: settings.endpoints.token.clone(),
            scopes: SCOPES.iter().map(|s| (*s).to_owned()).collect(),
            extra_authorize_params: vec![("token_access_type".into(), "offline".into())],
            redirect: settings.redirect.clone(),
        })
    }

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

    /// The account's roles, for callers that need the concrete type (tests).
    ///
    /// # Errors
    /// `Unsupported(List)` if the account isn't connected.
    pub fn open_account(&self, id: AccountId) -> Result<DropboxFs, VfsError> {
        let accounts = self.inner.accounts.read().unwrap_or_else(PoisonError::into_inner);
        let api = accounts.get(&id).cloned().ok_or(VfsError::Unsupported(Capability::List))?;
        let mount = DriveLocation { provider: DriveProvider::Dropbox, account: id }.mount()?;
        let endpoints = &self.inner.settings.endpoints;
        Ok(DropboxFs::new(api, endpoints.rpc.clone(), endpoints.content.clone(), StoragePath::root(mount)))
    }
}

impl std::fmt::Debug for DropboxService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DropboxService").finish_non_exhaustive()
    }
}

async fn whoami(api: &ApiClient, rpc: &str) -> Result<(String, String), VfsError> {
    let url = format!("{rpc}/2/users/get_current_account");
    let response =
        api.send(|http| http.post(&url).header(reqwest::header::CONTENT_TYPE, "application/json").body("null")).await?;
    if !response.status().is_success() {
        return Err(if response.status().as_u16() == 401 {
            VfsError::PermissionDenied
        } else {
            VfsError::Io { message: format!("Dropbox answered {}", response.status()) }
        });
    }
    let account: serde_json::Value = response.json().await.map_err(|e| network_error(&e))?;
    Ok((
        account["email"].as_str().unwrap_or_default().to_owned(),
        account["name"]["display_name"].as_str().unwrap_or_default().to_owned(),
    ))
}

impl DriveConnector for DropboxService {
    fn provider(&self) -> DriveProvider {
        DriveProvider::Dropbox
    }

    fn sign_in(&self, cancel: CancelToken) -> BoxFuture<'static, Result<SignedIn, SignInError>> {
        let service = self.clone();
        async move {
            let app = service.app().ok_or(SignInError::NotConfigured(DriveProvider::Dropbox))?;
            let tokens = authorize(&app, &service.inner.http, &*service.inner.opener, cancel).await?;
            let refresh = tokens.refresh_token.ok_or(SignInError::Denied("Dropbox sent no refresh token".into()))?;
            let api = service.client(app, refresh.clone(), None);
            let (email, display_name) = whoami(&api, &service.inner.settings.endpoints.rpc).await?;
            Ok(SignedIn { email, display_name, refresh_token: refresh })
        }
        .boxed()
    }

    fn connect(&self, account: &DriveAccount, refresh_token: Secret) -> BoxFuture<'static, Result<(), VfsError>> {
        let (service, id) = (self.clone(), account.id);
        async move {
            let app = service.app().ok_or(VfsError::Unsupported(Capability::List))?;
            let api = service.client(app, refresh_token, Some(id));
            whoami(&api, &service.inner.settings.endpoints.rpc).await?;
            service.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).insert(id, api);
            Ok(())
        }
        .boxed()
    }

    fn disconnect(&self, id: AccountId) {
        self.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }
}

impl ProviderFactory for DropboxService {
    fn scheme(&self) -> Scheme {
        self.inner.scheme.clone()
    }

    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        let location = DriveLocation::of(mount)
            .ok_or_else(|| VfsError::InvalidPath(PathError::InvalidAuthority(mount.authority().into())))?;
        let fs = Arc::new(self.open_account(location.account)?);
        Ok(MountHandle::builder(DROPBOX_SEMANTICS)
            .lister(fs.clone())
            .reader(fs.clone())
            .writer(fs.clone())
            .copier(fs.clone())
            .mover(fs.clone())
            .remover(fs.clone())
            .folder_maker(fs)
            .build())
    }
}
