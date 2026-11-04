use std::sync::Arc;

use domain::{DriveAccount, DriveProvider};

use crate::cancel::CancelToken;
use crate::ports::{
    CredentialError, CredentialKey, CredentialStore, DriveAccountStore, DriveConnector, NewDriveAccount, SignInError,
    StoreError,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DriveError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Keychain(#[from] CredentialError),
    #[error(transparent)]
    SignIn(#[from] SignInError),
    #[error("{0} isn't supported in this build")]
    NoProvider(DriveProvider),
    /// The account was saved without a token (keychain reset, item deleted).
    #[error("the sign-in for this account is missing; sign in again")]
    MissingToken,
}

/// The parts every drive use case needs.
#[derive(Clone)]
pub struct DriveServices {
    pub store: Arc<dyn DriveAccountStore>,
    pub credentials: Arc<dyn CredentialStore>,
    pub connectors: Vec<Arc<dyn DriveConnector>>,
}

impl DriveServices {
    fn connector(&self, provider: DriveProvider) -> Result<&Arc<dyn DriveConnector>, DriveError> {
        self.connectors.iter().find(|c| c.provider() == provider).ok_or(DriveError::NoProvider(provider))
    }
}

/// Signs in through the browser and saves the account only if it then
/// connects. On failure nothing is left behind.
pub struct AddDrive {
    services: DriveServices,
}

impl AddDrive {
    #[must_use]
    pub fn new(services: DriveServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// [`DriveError`] describing which step failed (`SignIn(Cancelled)` if
    /// the user gave up).
    pub async fn execute(&self, provider: DriveProvider, cancel: CancelToken) -> Result<DriveAccount, DriveError> {
        let s = &self.services;
        let connector = s.connector(provider)?;
        let signed_in = connector.sign_in(cancel).await?;
        let name = if signed_in.display_name.trim().is_empty() { provider.to_string() } else { signed_in.display_name };
        let added = s.store.add_drive(NewDriveAccount { provider, name, email: signed_in.email }).await?;
        let key = CredentialKey::drive_token(added.id);
        let result = async {
            s.credentials.store(&key, &signed_in.refresh_token).await?;
            connector.connect(&added, signed_in.refresh_token).await.map_err(|e| DriveError::SignIn(e.into()))
        }
        .await;
        match result {
            Ok(()) => Ok(added),
            Err(error) => {
                connector.disconnect(added.id);
                let _ = s.credentials.delete(&key).await;
                let _ = s.store.remove_drive(added.id).await;
                Err(error)
            }
        }
    }
}

pub struct ListDrives {
    store: Arc<dyn DriveAccountStore>,
}

impl ListDrives {
    #[must_use]
    pub fn new(store: Arc<dyn DriveAccountStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self) -> Result<Vec<DriveAccount>, StoreError> {
        self.store.list_drives().await
    }
}

/// Connects one saved drive with its stored token.
pub struct ConnectDrive {
    services: DriveServices,
}

impl ConnectDrive {
    #[must_use]
    pub fn new(services: DriveServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// [`DriveError`] describing which step failed.
    pub async fn execute(&self, account: &DriveAccount) -> Result<(), DriveError> {
        let s = &self.services;
        let connector = s.connector(account.provider)?;
        let token =
            s.credentials.load(&CredentialKey::drive_token(account.id)).await?.ok_or(DriveError::MissingToken)?;
        connector.connect(account, token).await.map_err(|e| DriveError::SignIn(e.into()))
    }
}

/// Forgets an account and its token. Revoking the app's access on the
/// provider's website is up to the user; the token stops being used here.
pub struct RemoveDrive {
    services: DriveServices,
}

impl RemoveDrive {
    #[must_use]
    pub fn new(services: DriveServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// [`DriveError`] if the store or keychain refuse.
    pub async fn execute(&self, account: &DriveAccount) -> Result<(), DriveError> {
        let s = &self.services;
        if let Ok(connector) = s.connector(account.provider) {
            connector.disconnect(account.id);
        }
        s.credentials.delete(&CredentialKey::drive_token(account.id)).await?;
        s.store.remove_drive(account.id).await?;
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

opaque_debug!(AddDrive, ListDrives, ConnectDrive, RemoveDrive);

impl std::fmt::Debug for DriveServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriveServices").field("connectors", &self.connectors.len()).finish_non_exhaustive()
    }
}
