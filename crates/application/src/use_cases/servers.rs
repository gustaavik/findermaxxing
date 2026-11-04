use std::sync::Arc;

use domain::{HostTrust, ServerAccount, ServerAuth, ServerProtocol};

use crate::ports::{
    ConnectError, CredentialError, CredentialKey, CredentialStore, NewServerAccount, Secret, ServerAccountStore,
    ServerConnector, StoreError,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServerError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Keychain(#[from] CredentialError),
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error("{0} servers are not supported in this build")]
    NoProvider(ServerProtocol),
    #[error("a password is needed")]
    MissingSecret,
}

/// The parts every server use case needs.
#[derive(Clone)]
pub struct ServerServices {
    pub store: Arc<dyn ServerAccountStore>,
    pub credentials: Arc<dyn CredentialStore>,
    /// One per protocol; adding a protocol is adding a connector (Open/Closed).
    pub connectors: Vec<Arc<dyn ServerConnector>>,
}

impl ServerServices {
    fn connector(&self, protocol: ServerProtocol) -> Result<&Arc<dyn ServerConnector>, ServerError> {
        self.connectors.iter().find(|c| c.protocol() == protocol).ok_or(ServerError::NoProvider(protocol))
    }
}

/// Adds a server only if it can be reached and signed in to. On failure
/// nothing is left behind. An SSH server seen for the first time fails with
/// `UnknownHost`; once the user has checked the key, adding again with
/// `HostTrust::Accept` remembers it.
pub struct AddServer {
    services: ServerServices,
}

impl AddServer {
    #[must_use]
    pub fn new(services: ServerServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// [`ServerError`] describing which step failed.
    pub async fn execute(
        &self,
        account: NewServerAccount,
        secret: Option<Secret>,
        trust: HostTrust,
    ) -> Result<ServerAccount, ServerError> {
        if account.auth == ServerAuth::Password && secret.is_none() {
            return Err(ServerError::MissingSecret);
        }
        let s = &self.services;
        let connector = s.connector(account.protocol)?;
        let secret = secret.filter(|_| account.auth.uses_secret());
        let added = s.store.add_server(account).await?;
        let key = CredentialKey::server_secret(added.id);
        let result = async {
            if let Some(secret) = &secret {
                s.credentials.store(&key, secret).await?;
            }
            connector.connect(&added, secret, trust).await.map_err(ServerError::Connect)
        }
        .await;
        match result {
            Ok(()) => Ok(added),
            Err(error) => {
                connector.disconnect(added.id);
                let _ = s.credentials.delete(&key).await;
                let _ = s.store.remove_server(added.id).await;
                Err(error)
            }
        }
    }
}

pub struct ListServers {
    store: Arc<dyn ServerAccountStore>,
}

impl ListServers {
    #[must_use]
    pub fn new(store: Arc<dyn ServerAccountStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self) -> Result<Vec<ServerAccount>, StoreError> {
        self.store.list_servers().await
    }
}

/// Connects one saved server with its stored secret, trusting only known
/// host keys. Callers connect accounts concurrently, so one slow server
/// never holds up the others.
pub struct ConnectServer {
    services: ServerServices,
}

impl ConnectServer {
    #[must_use]
    pub fn new(services: ServerServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// [`ServerError`] describing which step failed.
    pub async fn execute(&self, account: &ServerAccount) -> Result<(), ServerError> {
        let s = &self.services;
        let connector = s.connector(account.protocol)?;
        let secret = if account.auth.uses_secret() {
            let stored = s.credentials.load(&CredentialKey::server_secret(account.id)).await?;
            if account.auth == ServerAuth::Password && stored.is_none() {
                return Err(ServerError::MissingSecret);
            }
            stored
        } else {
            None
        };
        connector.connect(account, secret, HostTrust::KnownOnly).await.map_err(ServerError::Connect)
    }
}

pub struct RemoveServer {
    services: ServerServices,
}

impl RemoveServer {
    #[must_use]
    pub fn new(services: ServerServices) -> Self {
        Self { services }
    }

    /// Forgets settings and secret. Remembered host keys stay, like `~/.ssh/known_hosts`.
    ///
    /// # Errors
    /// [`ServerError`] if the store or keychain refuse.
    pub async fn execute(&self, account: &ServerAccount) -> Result<(), ServerError> {
        let s = &self.services;
        if let Ok(connector) = s.connector(account.protocol) {
            connector.disconnect(account.id);
        }
        s.credentials.delete(&CredentialKey::server_secret(account.id)).await?;
        s.store.remove_server(account.id).await?;
        Ok(())
    }
}

macro_rules! opaque_debug {
    ($($name:ident),+) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )+};
}

opaque_debug!(AddServer, ListServers, ConnectServer, RemoveServer);

impl std::fmt::Debug for ServerServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerServices").field("connectors", &self.connectors.len()).finish_non_exhaustive()
    }
}
