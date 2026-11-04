//! Sidebar sources: user favourites and mounted volumes.

use domain::StoragePath;
use futures::future::BoxFuture;

use super::preview::LocalFile;
use super::saved_searches::StoreError;
use domain::VfsError;

pub trait FavoritesStore: Send + Sync {
    /// In the user's order.
    fn list(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>>;
    /// Appends; adding an existing favourite does nothing.
    fn add(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>>;
    fn remove(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub name: String,
    pub location: StoragePath,
}

/// Disks, USB sticks and network shares mounted into the local filesystem.
pub trait VolumeLister: Send + Sync {
    fn volumes(&self) -> BoxFuture<'static, Vec<Volume>>;
}

/// Opens a local file with its default application (used for remote items
/// after they have been materialised).
pub trait LocalFileOpener: Send + Sync {
    fn open(&self, file: &LocalFile) -> BoxFuture<'static, Result<(), VfsError>>;
}
