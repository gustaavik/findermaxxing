//! `smb://` provider: Windows and NAS shares over SMB 2/3 (ADR 0021), on
//! the pure-Rust `smb2` client. Samba's libsmbclient is GPL and can't ship
//! in a closed-source app; shares the OS has mounted (Finder, gvfs) keep
//! appearing under Locations as local folders.
//!
//! Roles: `Lister`, `Reader`, `Writer` (hidden partial + no-replace rename,
//! resumable), `Copier` (streams), `Mover`, `Remover`, `FolderMaker`.

mod errors;
mod fs;

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use application::ports::{ConnectError, MountHandle, ProviderFactory, Secret, ServerConnector};
use domain::{
    AccountId, Capabilities, Capability, HostTrust, MountId, PathError, Scheme, ServerAccount, ServerAuth,
    ServerLocation, ServerProtocol, StoragePath, VfsError,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use smb2::{ClientConfig, SmbClient, Tree};
use tokio::runtime::Handle;
use tokio::sync::Mutex;

pub use fs::SmbFs;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SEPARATOR: char = '/';

const SMB_SEMANTICS: Capabilities =
    Capabilities::empty().with(Capability::AtomicRename).with(Capability::ResumableUpload);

struct Connected {
    client: Arc<Mutex<SmbClient>>,
    tree: Tree,
    base: String,
}

struct Inner {
    scheme: Scheme,
    runtime: Handle,
    accounts: RwLock<HashMap<AccountId, Connected>>,
}

/// Connected shares and the `smb://` provider factory. Cheap to clone.
#[derive(Clone)]
pub struct SmbService {
    inner: Arc<Inner>,
}

impl SmbService {
    /// # Errors
    /// Never in practice; the scheme literal is valid.
    pub fn new(runtime: Handle) -> Result<Self, PathError> {
        let scheme = ServerProtocol::Smb.scheme()?;
        Ok(Self { inner: Arc::new(Inner { scheme, runtime, accounts: RwLock::default() }) })
    }

    /// The account's roles, for callers that need the concrete type (tests).
    ///
    /// # Errors
    /// `Unsupported(List)` if the account isn't connected.
    pub fn open_account(&self, id: AccountId) -> Result<SmbFs, VfsError> {
        let accounts = self.inner.accounts.read().unwrap_or_else(PoisonError::into_inner);
        let connected = accounts.get(&id).ok_or(VfsError::Unsupported(Capability::List))?;
        let mount = ServerLocation { protocol: ServerProtocol::Smb, account: id }.mount()?;
        Ok(SmbFs::new(
            Arc::clone(&connected.client),
            connected.tree.clone(),
            connected.base.clone(),
            StoragePath::root(mount),
            self.inner.runtime.clone(),
        ))
    }
}

impl std::fmt::Debug for SmbService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmbService").finish_non_exhaustive()
    }
}

/// `share/folder/inside` → (`share`, `folder/inside`).
fn share_and_base(root: &str) -> Result<(String, String), VfsError> {
    let mut parts = root.split(SEPARATOR).filter(|part| !part.is_empty());
    let share = parts.next().ok_or(VfsError::NotFound)?.to_owned();
    Ok((share, parts.collect::<Vec<_>>().join("/")))
}

async fn open(account: &ServerAccount, secret: Option<Secret>) -> Result<Connected, VfsError> {
    let (share, base) = share_and_base(&account.root)?;
    let guest = account.auth == ServerAuth::Anonymous;
    let config = ClientConfig {
        addr: format!("{}:{}", account.host, account.port_or_default()),
        timeout: CONNECT_TIMEOUT,
        username: if guest { String::new() } else { account.username.clone() },
        password: secret.map(|secret| secret.expose().to_owned()).unwrap_or_default(),
        auto_reconnect: true,
        ..ClientConfig::default()
    };
    let mut client = SmbClient::connect(config).await.map_err(|e| errors::from_smb(&e))?;
    let mut tree = client.connect_share(&share).await.map_err(|e| errors::from_smb(&e))?;
    if !base.is_empty() {
        let info = client.stat(&mut tree, &base).await.map_err(|e| errors::from_smb(&e))?;
        if !info.is_directory {
            return Err(VfsError::NotADirectory);
        }
    }
    Ok(Connected { client: Arc::new(Mutex::new(client)), tree, base })
}

impl ServerConnector for SmbService {
    fn protocol(&self) -> ServerProtocol {
        ServerProtocol::Smb
    }

    /// SMB 3 signs (and, if the server wants, encrypts) every message;
    /// there is no host key to confirm.
    fn connect(
        &self,
        account: &ServerAccount,
        secret: Option<Secret>,
        _trust: HostTrust,
    ) -> BoxFuture<'static, Result<(), ConnectError>> {
        let (service, account) = (self.clone(), account.clone());
        async move {
            let connected = open(&account, secret).await?;
            service.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).insert(account.id, connected);
            Ok(())
        }
        .boxed()
    }

    fn disconnect(&self, id: AccountId) {
        self.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }
}

impl ProviderFactory for SmbService {
    fn scheme(&self) -> Scheme {
        self.inner.scheme.clone()
    }

    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        let location = ServerLocation::of(mount)
            .ok_or_else(|| VfsError::InvalidPath(PathError::InvalidAuthority(mount.authority().into())))?;
        let fs = Arc::new(self.open_account(location.account)?);
        Ok(MountHandle::builder(SMB_SEMANTICS)
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
