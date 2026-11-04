//! Roles that only some providers have: editable metadata, share links and
//! version history. Object-safe directly; each is used through `dyn` only.

use std::collections::BTreeMap;
use std::time::Duration;

use domain::{ByteSize, StoragePath, Timestamp, VfsError};
use futures::future::BoxFuture;

/// Editable and read-only facts about one item.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemProperties {
    pub content_type: Option<String>,
    pub user_metadata: BTreeMap<String, String>,
    /// Read-only, provider-specific facts, already labelled ("ETag", "Storage class", …).
    pub facts: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PropertyChanges {
    pub content_type: Option<String>,
    pub user_metadata: BTreeMap<String, String>,
}

pub trait MetadataEditor: Send + Sync {
    fn properties<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<ItemProperties, VfsError>>;

    fn update<'a>(&'a self, path: &'a StoragePath, changes: &'a PropertyChanges)
    -> BoxFuture<'a, Result<(), VfsError>>;
}

pub trait Presigner: Send + Sync {
    /// A link anyone can download the item with, valid for `valid_for`.
    fn share_link<'a>(&'a self, path: &'a StoragePath, valid_for: Duration) -> BoxFuture<'a, Result<String, VfsError>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemVersion {
    pub version_id: String,
    pub modified: Option<Timestamp>,
    pub size: Option<ByteSize>,
    pub is_latest: bool,
    pub is_delete_marker: bool,
}

pub trait VersionLister: Send + Sync {
    /// Newest first.
    fn versions<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<Vec<ItemVersion>, VfsError>>;
}
