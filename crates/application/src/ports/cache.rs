//! The local cache of remote files and the folders kept available offline.

use domain::{AccountId, ByteSize, FileEntry, MountId, S3Location, ServerLocation, StoragePath, Timestamp};
use futures::future::BoxFuture;

use super::saved_searches::StoreError;

/// One cached copy of a remote file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRecord {
    pub location: StoragePath,
    /// Path of the copy, relative to the cache root.
    pub file: String,
    pub size: ByteSize,
    /// The remote item's modification time when it was downloaded.
    pub remote_modified: Option<Timestamp>,
    pub last_used: Timestamp,
}

/// Bookkeeping for the cache; the copies themselves live on disk.
pub trait CacheIndex: Send + Sync {
    fn lookup(&self, location: &StoragePath) -> BoxFuture<'static, Result<Option<CacheRecord>, StoreError>>;
    fn record(&self, record: CacheRecord) -> BoxFuture<'static, Result<(), StoreError>>;
    fn touch(&self, location: &StoragePath, at: Timestamp) -> BoxFuture<'static, Result<(), StoreError>>;
    fn remove(&self, locations: Vec<StoragePath>) -> BoxFuture<'static, Result<(), StoreError>>;
    fn all(&self) -> BoxFuture<'static, Result<Vec<CacheRecord>, StoreError>>;

    /// The last listing of a folder kept offline, for browsing without a connection.
    fn save_listing(&self, folder: &StoragePath, entries: Vec<FileEntry>)
    -> BoxFuture<'static, Result<(), StoreError>>;
    fn listing(&self, folder: &StoragePath) -> BoxFuture<'static, Result<Option<Vec<FileEntry>>, StoreError>>;
    /// Every folder with a stored listing.
    fn listed_folders(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>>;
    fn remove_listings(&self, folders: Vec<StoragePath>) -> BoxFuture<'static, Result<(), StoreError>>;
}

/// The mounts one account provides: a server is one mount, an S3 account
/// one per bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountMounts {
    Server(ServerLocation),
    S3(AccountId),
}

impl AccountMounts {
    #[must_use]
    pub fn contains(&self, mount: &MountId) -> bool {
        match self {
            Self::Server(server) => ServerLocation::of(mount) == Some(*server),
            Self::S3(account) => S3Location::of(mount).is_some_and(|location| location.account == *account),
        }
    }
}

/// Deletes everything kept locally for an account: cached copies, stored
/// listings and offline pins. Used when an account is removed.
pub trait CacheJanitor: Send + Sync {
    fn forget(&self, account: AccountMounts) -> BoxFuture<'static, Result<(), StoreError>>;
}

/// Items the user keeps available offline. Pinning a folder pins everything in it.
pub trait PinStore: Send + Sync {
    fn pins(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>>;
    fn pin(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>>;
    fn unpin(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>>;
}

/// Whether `location` is pinned itself or lies inside a pinned folder.
#[must_use]
pub fn is_pinned(pins: &[StoragePath], location: &StoragePath) -> bool {
    pins.iter().any(|pin| location.is_within(pin))
}
