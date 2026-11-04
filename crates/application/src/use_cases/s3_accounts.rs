use std::sync::Arc;

use domain::{AccountId, BucketName, S3Account, S3Auth, VfsError};

use crate::ports::{
    AccountStore, BucketInfo, CredentialError, CredentialKey, CredentialStore, NewS3Account, S3Connector, Secret,
    StoreError,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Keychain(#[from] CredentialError),
    #[error("couldn't connect: {0}")]
    Connection(VfsError),
    #[error("an access key needs its secret")]
    MissingSecret,
}

/// The parts every account use case needs.
#[derive(Clone)]
pub struct AccountServices {
    pub store: Arc<dyn AccountStore>,
    pub credentials: Arc<dyn CredentialStore>,
    pub connector: Arc<dyn S3Connector>,
}

/// Adds an account only if it works: it must list its buckets. On failure
/// nothing is left behind (no stored settings, no keychain item).
pub struct AddS3Account {
    services: AccountServices,
}

impl AddS3Account {
    #[must_use]
    pub fn new(services: AccountServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// [`AccountError`] describing which step failed.
    pub async fn execute(
        &self,
        account: NewS3Account,
        secret: Option<Secret>,
    ) -> Result<(S3Account, Vec<BucketInfo>), AccountError> {
        if matches!(account.auth, S3Auth::AccessKey { .. }) && secret.is_none() {
            return Err(AccountError::MissingSecret);
        }
        let s = &self.services;
        let added = s.store.add_s3(account).await?;
        let key = CredentialKey::s3_secret(added.id);
        let result = async {
            if let Some(secret) = &secret {
                s.credentials.store(&key, secret).await?;
            }
            s.connector.connect(&added, secret).await.map_err(AccountError::Connection)?;
            s.connector.list_buckets(added.id).await.map_err(AccountError::Connection)
        }
        .await;
        match result {
            Ok(buckets) => Ok((added, buckets)),
            Err(error) => {
                s.connector.disconnect(added.id);
                let _ = s.credentials.delete(&key).await;
                let _ = s.store.remove(added.id).await;
                Err(error)
            }
        }
    }
}

/// How each stored account fared when connecting at start-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedAccount {
    pub account: S3Account,
    pub status: Result<(), AccountError>,
}

/// Loads stored accounts and their secrets and connects them. A failing
/// account (offline, revoked key) is reported, never fatal.
pub struct ConnectSavedAccounts {
    services: AccountServices,
}

impl ConnectSavedAccounts {
    #[must_use]
    pub fn new(services: AccountServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// Only if the account list itself can't be read.
    pub async fn execute(&self) -> Result<Vec<ConnectedAccount>, AccountError> {
        let s = &self.services;
        let mut connected = Vec::new();
        for account in s.store.list_s3().await? {
            let status = async {
                let secret = match account.auth {
                    S3Auth::AccessKey { .. } => Some(
                        s.credentials
                            .load(&CredentialKey::s3_secret(account.id))
                            .await?
                            .ok_or(AccountError::MissingSecret)?,
                    ),
                    S3Auth::DefaultChain | S3Auth::Profile(_) => None,
                };
                s.connector.connect(&account, secret).await.map_err(AccountError::Connection)
            }
            .await;
            connected.push(ConnectedAccount { account, status });
        }
        Ok(connected)
    }
}

pub struct ListS3Accounts {
    store: Arc<dyn AccountStore>,
}

impl ListS3Accounts {
    #[must_use]
    pub fn new(store: Arc<dyn AccountStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self) -> Result<Vec<S3Account>, StoreError> {
        self.store.list_s3().await
    }
}

pub struct RemoveS3Account {
    services: AccountServices,
}

impl RemoveS3Account {
    #[must_use]
    pub fn new(services: AccountServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// Store or keychain failures. The account is disconnected regardless.
    pub async fn execute(&self, id: AccountId) -> Result<(), AccountError> {
        let s = &self.services;
        s.connector.disconnect(id);
        s.credentials.delete(&CredentialKey::s3_secret(id)).await?;
        s.store.remove(id).await?;
        Ok(())
    }
}

/// The account's buckets: those it can list, plus those added by name.
pub struct ListBuckets {
    connector: Arc<dyn S3Connector>,
    store: Arc<dyn AccountStore>,
}

impl ListBuckets {
    #[must_use]
    pub fn new(connector: Arc<dyn S3Connector>, store: Arc<dyn AccountStore>) -> Self {
        Self { connector, store }
    }

    /// Listing may be denied (scoped keys); buckets added by name still show.
    ///
    /// # Errors
    /// Provider errors when there is nothing to show at all.
    pub async fn execute(&self, id: AccountId) -> Result<Vec<BucketInfo>, VfsError> {
        let extra: Vec<BucketName> = self
            .store
            .list_s3()
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.id == id)
            .map(|a| a.extra_buckets)
            .unwrap_or_default();
        let listed = match self.connector.list_buckets(id).await {
            Ok(listed) => listed,
            Err(VfsError::PermissionDenied) if !extra.is_empty() => Vec::new(),
            Err(error) => return Err(error),
        };
        let mut buckets = listed;
        for name in extra {
            if !buckets.iter().any(|b| b.name == name) {
                buckets.push(BucketInfo { name, region: None });
            }
        }
        buckets.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(buckets)
    }
}

/// Adds a bucket by name, for keys that may not list buckets.
pub struct AddBucket {
    store: Arc<dyn AccountStore>,
}

impl AddBucket {
    #[must_use]
    pub fn new(store: Arc<dyn AccountStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, id: AccountId, bucket: BucketName) -> Result<(), StoreError> {
        self.store.add_bucket(id, bucket).await
    }
}

/// Records "don't ask again" for cost confirmations of an account.
pub struct SetCostWarnings {
    store: Arc<dyn AccountStore>,
}

impl SetCostWarnings {
    #[must_use]
    pub fn new(store: Arc<dyn AccountStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, id: AccountId, skip: bool) -> Result<(), StoreError> {
        self.store.set_skip_cost_warnings(id, skip).await
    }
}

macro_rules! opaque_debug {
    ($($type:ident),+) => {
        $(impl std::fmt::Debug for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($type)).finish_non_exhaustive()
            }
        })+
    };
}

opaque_debug!(
    AccountServices,
    AddS3Account,
    AddBucket,
    ListS3Accounts,
    ConnectSavedAccounts,
    RemoveS3Account,
    ListBuckets,
    SetCostWarnings
);
