//! `dav://` provider: WebDAV (RFC 4918) over HTTP(S) for Nextcloud, ownCloud,
//! NAS boxes and Apache `mod_dav` (ADR 0021).
//!
//! Roles: `Lister` (PROPFIND), `Reader` (GET with ranges), `Writer` (PUT to
//! a hidden partial, then MOVE without overwrite), `Copier` (server-side
//! COPY), `Mover` (MOVE), `Remover` (DELETE), `FolderMaker` (MKCOL).
//! Every write uses `Overwrite: F`, so nothing is ever replaced silently.

mod fs;
mod multistatus;
mod urls;

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use application::ports::{ConnectError, MountHandle, ProviderFactory, Secret, ServerConnector};
use domain::{
    AccountId, Capabilities, Capability, HostTrust, MountId, PathError, Scheme, ServerAccount, ServerLocation,
    ServerProtocol, StoragePath, Transport, VfsError,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::runtime::Handle;

pub use fs::DavFs;
use fs::{Connection, network_error};
use urls::Base;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT: &str = concat!("FinderMaxxing/", env!("CARGO_PKG_VERSION"));

const DAV_SEMANTICS: Capabilities =
    Capabilities::empty().with(Capability::AtomicRename).with(Capability::ServerSideCopy);

struct Inner {
    scheme: Scheme,
    runtime: Handle,
    accounts: RwLock<HashMap<AccountId, Connection>>,
}

/// Connected accounts and the `dav://` provider factory. Cheap to clone.
#[derive(Clone)]
pub struct WebDavService {
    inner: Arc<Inner>,
}

impl WebDavService {
    /// # Errors
    /// Never in practice; the scheme literal is valid.
    pub fn new(runtime: Handle) -> Result<Self, PathError> {
        let scheme = ServerProtocol::WebDav.scheme()?;
        Ok(Self { inner: Arc::new(Inner { scheme, runtime, accounts: RwLock::default() }) })
    }

    /// The account's roles, for callers that need the concrete type (tests).
    ///
    /// # Errors
    /// `Unsupported(List)` if the account isn't connected.
    pub fn open_account(&self, id: AccountId) -> Result<DavFs, VfsError> {
        let accounts = self.inner.accounts.read().unwrap_or_else(PoisonError::into_inner);
        let connection = accounts.get(&id).cloned().ok_or(VfsError::Unsupported(Capability::List))?;
        let mount = ServerLocation { protocol: ServerProtocol::WebDav, account: id }.mount()?;
        Ok(DavFs::new(connection, StoragePath::root(mount), self.inner.runtime.clone()))
    }
}

impl std::fmt::Debug for WebDavService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavService").finish_non_exhaustive()
    }
}

fn connection(account: &ServerAccount, secret: Option<Secret>) -> Result<Connection, VfsError> {
    let http = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| network_error(&error))?;
    let scheme = match account.transport {
        Transport::Tls => "https",
        Transport::Plain => "http",
    };
    let origin = format!("{scheme}://{}:{}", account.host, account.port_or_default());
    Ok(Connection {
        http,
        base: Base::new(origin, &account.root),
        username: account.username.clone(),
        password: secret.map(|secret| secret.expose().to_owned()),
    })
}

impl ServerConnector for WebDavService {
    fn protocol(&self) -> ServerProtocol {
        ServerProtocol::WebDav
    }

    /// Checks the root can be listed with these credentials. TLS
    /// certificates are verified against the OS trust store.
    fn connect(
        &self,
        account: &ServerAccount,
        secret: Option<Secret>,
        _trust: HostTrust,
    ) -> BoxFuture<'static, Result<(), ConnectError>> {
        let (service, account) = (self.clone(), account.clone());
        async move {
            let connection = connection(&account, secret)?;
            let root = StoragePath::root(account.mount().map_err(VfsError::from)?);
            connection.check_root(&root).await?;
            service.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).insert(account.id, connection);
            Ok(())
        }
        .boxed()
    }

    fn disconnect(&self, id: AccountId) {
        self.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }
}

impl ProviderFactory for WebDavService {
    fn scheme(&self) -> Scheme {
        self.inner.scheme.clone()
    }

    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        let location = ServerLocation::of(mount)
            .ok_or_else(|| VfsError::InvalidPath(PathError::InvalidAuthority(mount.authority().into())))?;
        let fs = Arc::new(self.open_account(location.account)?);
        Ok(MountHandle::builder(DAV_SEMANTICS)
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
