//! File-server accounts (SFTP, WebDAV, SMB) and connecting them.

use domain::{AccountId, HostKey, HostTrust, ServerAccount, ServerAuth, ServerProtocol, Transport, VfsError};
use futures::future::BoxFuture;

use super::accounts::Secret;
use super::saved_searches::StoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewServerAccount {
    pub protocol: ServerProtocol,
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub transport: Transport,
    pub username: String,
    pub root: String,
    pub auth: ServerAuth,
}

impl NewServerAccount {
    #[must_use]
    pub fn with_id(self, id: AccountId) -> ServerAccount {
        ServerAccount {
            id,
            protocol: self.protocol,
            name: self.name,
            host: self.host,
            port: self.port,
            transport: self.transport,
            username: self.username,
            root: self.root,
            auth: self.auth,
        }
    }
}

/// Server settings (never secrets).
pub trait ServerAccountStore: Send + Sync {
    fn list_servers(&self) -> BoxFuture<'static, Result<Vec<ServerAccount>, StoreError>>;
    fn add_server(&self, account: NewServerAccount) -> BoxFuture<'static, Result<ServerAccount, StoreError>>;
    fn remove_server(&self, id: AccountId) -> BoxFuture<'static, Result<(), StoreError>>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Failed(#[from] VfsError),
    /// First contact: the user must confirm the server's key.
    #[error("the server's identity ({}) is not known yet", .0.fingerprint)]
    UnknownHost(HostKey),
    /// The key differs from the one remembered: possibly an attack.
    #[error("the server's identity changed (now {})", .0.fingerprint)]
    HostKeyChanged(HostKey),
}

/// Makes server accounts usable by their provider. One per protocol.
pub trait ServerConnector: Send + Sync {
    fn protocol(&self) -> ServerProtocol;

    fn connect(
        &self,
        account: &ServerAccount,
        secret: Option<Secret>,
        trust: HostTrust,
    ) -> BoxFuture<'static, Result<(), ConnectError>>;

    fn disconnect(&self, id: AccountId);
}
