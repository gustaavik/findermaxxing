//! `onedrive://` provider: OneDrive through Microsoft Graph (ADR 0024).
//!
//! Roles: `Lister`, `Reader` (ranged, via pre-authorised download URLs),
//! `Writer` (one request for small files, a resumable upload session for
//! large ones, both `conflictBehavior: fail`), `Copier` (asynchronous,
//! server-side), `Mover`, `Remover` (to the OneDrive recycle bin),
//! `FolderMaker`. Personal and work/school accounts (`common` tenant).

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

pub use fs::OneDriveFs;

/// Microsoft registers `http://localhost`; any port is accepted for it.
const LOOPBACK_HOST: &str = "localhost";
const SCOPES: &[&str] = &["Files.ReadWrite.All", "User.Read", "offline_access"];

const ONEDRIVE_SEMANTICS: Capabilities = Capabilities::empty()
    .with(Capability::AtomicRename)
    .with(Capability::ServerSideCopy)
    .with(Capability::ResumableUpload);

#[derive(Debug, Clone)]
pub struct OneDriveEndpoints {
    pub graph: String,
    pub authorize: String,
    pub token: String,
}

impl Default for OneDriveEndpoints {
    fn default() -> Self {
        Self {
            graph: "https://graph.microsoft.com/v1.0".into(),
            authorize: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
        }
    }
}

/// This build's app registration ("Mobile and desktop applications"
/// platform, redirect `http://localhost`; public client, no secret).
#[derive(Debug, Clone)]
pub struct OneDriveSettings {
    pub client_id: Option<String>,
    pub endpoints: OneDriveEndpoints,
    pub redirect: Redirect,
}

impl Default for OneDriveSettings {
    fn default() -> Self {
        Self {
            client_id: None,
            endpoints: OneDriveEndpoints::default(),
            redirect: Redirect::AnyPort { host: LOOPBACK_HOST },
        }
    }
}

struct Inner {
    scheme: Scheme,
    runtime: Handle,
    settings: OneDriveSettings,
    http: reqwest::Client,
    opener: Arc<dyn UrlOpener>,
    credentials: Arc<dyn CredentialStore>,
    accounts: RwLock<HashMap<AccountId, ApiClient>>,
}

/// Connected accounts and the `dropbox://` provider factory. Cheap to clone.
#[derive(Clone)]
pub struct OneDriveService {
    inner: Arc<Inner>,
}

impl OneDriveService {
    /// # Errors
    /// Never in practice; the scheme literal is valid.
    pub fn new(
        runtime: Handle,
        settings: OneDriveSettings,
        opener: Arc<dyn UrlOpener>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, PathError> {
        let inner = Inner {
            scheme: DriveProvider::OneDrive.scheme()?,
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
            extra_authorize_params: Vec::new(),
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
    pub fn open_account(&self, id: AccountId) -> Result<OneDriveFs, VfsError> {
        let accounts = self.inner.accounts.read().unwrap_or_else(PoisonError::into_inner);
        let api = accounts.get(&id).cloned().ok_or(VfsError::Unsupported(Capability::List))?;
        let mount = DriveLocation { provider: DriveProvider::OneDrive, account: id }.mount()?;
        let graph = self.inner.settings.endpoints.graph.clone();
        Ok(OneDriveFs::new(api, graph, StoragePath::root(mount), self.inner.runtime.clone()))
    }
}

impl std::fmt::Debug for OneDriveService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OneDriveService").finish_non_exhaustive()
    }
}

async fn whoami(api: &ApiClient, graph: &str) -> Result<(String, String), VfsError> {
    let url = format!("{graph}/me");
    let response = api.send(|http| http.get(&url)).await?;
    if !response.status().is_success() {
        return Err(if response.status().as_u16() == 401 {
            VfsError::PermissionDenied
        } else {
            VfsError::Io { message: format!("Microsoft answered {}", response.status()) }
        });
    }
    let me: serde_json::Value = response.json().await.map_err(|e| network_error(&e))?;
    let email = me["mail"].as_str().or_else(|| me["userPrincipalName"].as_str()).unwrap_or_default().to_owned();
    Ok((email, me["displayName"].as_str().unwrap_or_default().to_owned()))
}

impl DriveConnector for OneDriveService {
    fn provider(&self) -> DriveProvider {
        DriveProvider::OneDrive
    }

    fn sign_in(&self, cancel: CancelToken) -> BoxFuture<'static, Result<SignedIn, SignInError>> {
        let service = self.clone();
        async move {
            let app = service.app().ok_or(SignInError::NotConfigured(DriveProvider::OneDrive))?;
            let tokens = authorize(&app, &service.inner.http, &*service.inner.opener, cancel).await?;
            let refresh = tokens.refresh_token.ok_or(SignInError::Denied("Microsoft sent no refresh token".into()))?;
            let api = service.client(app, refresh.clone(), None);
            let (email, display_name) = whoami(&api, &service.inner.settings.endpoints.graph).await?;
            Ok(SignedIn { email, display_name, refresh_token: refresh })
        }
        .boxed()
    }

    fn connect(&self, account: &DriveAccount, refresh_token: Secret) -> BoxFuture<'static, Result<(), VfsError>> {
        let (service, id) = (self.clone(), account.id);
        async move {
            let app = service.app().ok_or(VfsError::Unsupported(Capability::List))?;
            let api = service.client(app, refresh_token, Some(id));
            whoami(&api, &service.inner.settings.endpoints.graph).await?;
            service.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).insert(id, api);
            Ok(())
        }
        .boxed()
    }

    fn disconnect(&self, id: AccountId) {
        self.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }
}

impl ProviderFactory for OneDriveService {
    fn scheme(&self) -> Scheme {
        self.inner.scheme.clone()
    }

    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        let location = DriveLocation::of(mount)
            .ok_or_else(|| VfsError::InvalidPath(PathError::InvalidAuthority(mount.authority().into())))?;
        let fs = Arc::new(self.open_account(location.account)?);
        Ok(MountHandle::builder(ONEDRIVE_SEMANTICS)
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
