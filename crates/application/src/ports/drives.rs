//! Cloud drives signed in with OAuth (Google Drive, Dropbox, OneDrive).

use domain::{AccountId, DriveAccount, DriveProvider, VfsError};
use futures::future::BoxFuture;

use super::accounts::Secret;
use super::saved_searches::StoreError;
use crate::cancel::CancelToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDriveAccount {
    pub provider: DriveProvider,
    pub name: String,
    pub email: String,
}

impl NewDriveAccount {
    #[must_use]
    pub fn with_id(self, id: AccountId) -> DriveAccount {
        DriveAccount { id, provider: self.provider, name: self.name, email: self.email }
    }
}

/// Drive settings (never tokens).
pub trait DriveAccountStore: Send + Sync {
    fn list_drives(&self) -> BoxFuture<'static, Result<Vec<DriveAccount>, StoreError>>;
    fn add_drive(&self, account: NewDriveAccount) -> BoxFuture<'static, Result<DriveAccount, StoreError>>;
    fn remove_drive(&self, id: AccountId) -> BoxFuture<'static, Result<(), StoreError>>;
}

/// The outcome of a browser sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    pub email: String,
    pub display_name: String,
    /// Kept in the keychain; access tokens are derived from it and never stored.
    pub refresh_token: Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignInError {
    #[error("signing in was cancelled")]
    Cancelled,
    /// This build has no OAuth client registered for the provider.
    #[error("{0} isn't set up in this build")]
    NotConfigured(DriveProvider),
    /// The user declined, or the provider refused the app.
    #[error("access wasn't granted: {0}")]
    Denied(String),
    #[error(transparent)]
    Failed(#[from] VfsError),
}

/// Makes drive accounts usable by their provider. One per provider.
pub trait DriveConnector: Send + Sync {
    fn provider(&self) -> DriveProvider;

    /// Opens the provider's sign-in page in the browser and waits for the
    /// user to come back (OAuth 2.0 authorization code with PKCE).
    fn sign_in(&self, cancel: CancelToken) -> BoxFuture<'static, Result<SignedIn, SignInError>>;

    /// Connects a saved account. Providers that rotate refresh tokens save
    /// the new one themselves.
    fn connect(&self, account: &DriveAccount, refresh_token: Secret) -> BoxFuture<'static, Result<(), VfsError>>;

    fn disconnect(&self, id: AccountId);
}

/// Opens a web page in the user's browser.
pub trait UrlOpener: Send + Sync {
    fn open_url(&self, url: &str) -> BoxFuture<'static, Result<(), VfsError>>;
}
