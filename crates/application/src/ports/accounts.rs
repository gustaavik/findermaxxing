//! Remote accounts, their secrets, and connecting them to their provider.

use std::fmt;

use domain::{AccountId, Addressing, BucketName, S3Account, S3Auth, VfsError};
use futures::future::BoxFuture;

use super::saved_searches::StoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewS3Account {
    pub name: String,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub auth: S3Auth,
    pub addressing: Addressing,
}

/// Account settings (never secrets).
pub trait AccountStore: Send + Sync {
    fn list_s3(&self) -> BoxFuture<'static, Result<Vec<S3Account>, StoreError>>;
    fn add_s3(&self, account: NewS3Account) -> BoxFuture<'static, Result<S3Account, StoreError>>;
    fn remove(&self, id: AccountId) -> BoxFuture<'static, Result<(), StoreError>>;
    fn set_skip_cost_warnings(&self, id: AccountId, skip: bool) -> BoxFuture<'static, Result<(), StoreError>>;
    /// Remembers a bucket the account can't list but can open by name.
    fn add_bucket(&self, id: AccountId, bucket: BucketName) -> BoxFuture<'static, Result<(), StoreError>>;
}

/// A secret value. Its `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Where a secret is filed in the OS keychain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialKey {
    pub service: String,
    pub account: String,
}

impl CredentialKey {
    const S3_SERVICE: &'static str = "FinderMaxxing S3";
    const SERVER_SERVICE: &'static str = "FinderMaxxing Servers";
    const DRIVE_SERVICE: &'static str = "FinderMaxxing Drives";

    #[must_use]
    pub fn s3_secret(id: AccountId) -> Self {
        Self { service: Self::S3_SERVICE.to_owned(), account: id.to_string() }
    }

    /// The OAuth refresh token of a cloud-drive account.
    #[must_use]
    pub fn drive_token(id: AccountId) -> Self {
        Self { service: Self::DRIVE_SERVICE.to_owned(), account: id.to_string() }
    }

    /// The password or key passphrase of a file-server account.
    #[must_use]
    pub fn server_secret(id: AccountId) -> Self {
        Self { service: Self::SERVER_SERVICE.to_owned(), account: id.to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the system keychain is unavailable: {0}")]
pub struct CredentialError(pub String);

/// The OS keychain (macOS Keychain, Secret Service on Linux).
pub trait CredentialStore: Send + Sync {
    fn store(&self, key: &CredentialKey, secret: &Secret) -> BoxFuture<'static, Result<(), CredentialError>>;
    fn load(&self, key: &CredentialKey) -> BoxFuture<'static, Result<Option<Secret>, CredentialError>>;
    fn delete(&self, key: &CredentialKey) -> BoxFuture<'static, Result<(), CredentialError>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketInfo {
    pub name: BucketName,
    pub region: Option<String>,
}

/// Makes accounts usable by the `s3://` provider and lists their buckets.
pub trait S3Connector: Send + Sync {
    fn connect(&self, account: &S3Account, secret: Option<Secret>) -> BoxFuture<'static, Result<(), VfsError>>;
    fn disconnect(&self, id: AccountId);
    fn list_buckets(&self, id: AccountId) -> BoxFuture<'static, Result<Vec<BucketInfo>, VfsError>>;
}
