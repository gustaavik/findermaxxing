//! `sftp://` provider: files over SSH (ADR 0021), on `russh` + `russh-sftp`
//! (pure Rust, Apache-2.0).
//!
//! Roles: `Lister`, `Reader` (seekable), `Writer` (hidden partial + rename,
//! resumable), `Mover` (rename, never replacing), `Remover` (recursive) and
//! `FolderMaker`, and a `Copier` that streams through this machine (SFTP v3
//! has no server-side copy).
//! Sign-in: password, SSH agent, or key file (passphrase in the keychain).

mod connect;
mod errors;
mod fs;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use application::ports::{ConnectError, MountHandle, ProviderFactory, Secret, ServerConnector};
use domain::{
    AccountId, Capabilities, Capability, HostTrust, MountId, PathError, Scheme, ServerAccount, ServerLocation,
    ServerProtocol, StoragePath, VfsError,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::runtime::Handle;

pub use fs::SftpFs;

const CURRENT_FOLDER: &str = ".";

const SFTP_SEMANTICS: Capabilities = Capabilities::empty()
    .with(Capability::AtomicRename)
    .with(Capability::PosixPermissions)
    .with(Capability::Symlinks)
    .with(Capability::RandomAccessRead)
    .with(Capability::ResumableUpload);

struct Connected {
    session: Arc<connect::Session>,
    root_path: String,
}

struct Inner {
    scheme: Scheme,
    runtime: Handle,
    known_hosts: PathBuf,
    accounts: RwLock<HashMap<AccountId, Connected>>,
}

/// Connected accounts and the `sftp://` provider factory. Cheap to clone.
#[derive(Clone)]
pub struct SftpService {
    inner: Arc<Inner>,
}

impl SftpService {
    /// `known_hosts` is the app's own file of accepted host keys
    /// (`~/.ssh/known_hosts` is read too, never written).
    ///
    /// # Errors
    /// Never in practice; the scheme literal is valid.
    pub fn new(runtime: Handle, known_hosts: PathBuf) -> Result<Self, PathError> {
        let scheme = ServerProtocol::Sftp.scheme()?;
        Ok(Self { inner: Arc::new(Inner { scheme, runtime, known_hosts, accounts: RwLock::default() }) })
    }

    /// The account's roles, for callers that need the concrete type (tests).
    ///
    /// # Errors
    /// `Unsupported(List)` if the account isn't connected.
    pub fn open_account(&self, id: AccountId) -> Result<SftpFs, VfsError> {
        let accounts = self.inner.accounts.read().unwrap_or_else(PoisonError::into_inner);
        let connected = accounts.get(&id).ok_or(VfsError::Unsupported(Capability::List))?;
        let mount = ServerLocation { protocol: ServerProtocol::Sftp, account: id }.mount()?;
        Ok(SftpFs::new(
            Arc::clone(&connected.session),
            connected.root_path.clone(),
            StoragePath::root(mount),
            self.inner.runtime.clone(),
        ))
    }
}

impl std::fmt::Debug for SftpService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpService").finish_non_exhaustive()
    }
}

impl ServerConnector for SftpService {
    fn protocol(&self) -> ServerProtocol {
        ServerProtocol::Sftp
    }

    fn connect(
        &self,
        account: &ServerAccount,
        secret: Option<Secret>,
        trust: HostTrust,
    ) -> BoxFuture<'static, Result<(), ConnectError>> {
        let (service, account) = (self.clone(), account.clone());
        async move {
            let session = connect::open(&account, secret, trust, &service.inner.known_hosts).await?;
            let start = if account.root.trim().is_empty() { CURRENT_FOLDER } else { account.root.trim() };
            let canonical =
                session.sftp.canonicalize(start).await.map_err(|e| ConnectError::Failed(errors::from_sftp(&e)))?;
            let root_path = canonical.trim_end_matches('/').to_owned();
            let connected = Connected { session: Arc::new(session), root_path };
            service.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).insert(account.id, connected);
            Ok(())
        }
        .boxed()
    }

    fn disconnect(&self, id: AccountId) {
        self.inner.accounts.write().unwrap_or_else(PoisonError::into_inner).remove(&id);
    }
}

impl ProviderFactory for SftpService {
    fn scheme(&self) -> Scheme {
        self.inner.scheme.clone()
    }

    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        let location = ServerLocation::of(mount)
            .ok_or_else(|| VfsError::InvalidPath(PathError::InvalidAuthority(mount.authority().into())))?;
        let fs = Arc::new(self.open_account(location.account)?);
        Ok(MountHandle::builder(SFTP_SEMANTICS)
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
