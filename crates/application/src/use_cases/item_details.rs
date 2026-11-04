use std::sync::Arc;
use std::time::Duration;

use domain::{StoragePath, VfsError};

use crate::ports::{ItemProperties, ItemVersion, MountResolver, PropertyChanges};

/// Share links are capped at the S3 maximum of seven days.
pub const MAX_SHARE_LINK_VALIDITY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub struct GetItemProperties {
    mounts: Arc<dyn MountResolver>,
}

impl GetItemProperties {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// # Errors
    /// `Unsupported(EditMetadata)` on providers without metadata.
    pub async fn execute(&self, item: &StoragePath) -> Result<ItemProperties, VfsError> {
        self.mounts.resolve(item.mount())?.metadata_editor()?.properties(item).await
    }
}

pub struct UpdateItemProperties {
    mounts: Arc<dyn MountResolver>,
}

impl UpdateItemProperties {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// # Errors
    /// Provider errors; `Unsupported(EditMetadata)` where metadata can't be edited.
    pub async fn execute(&self, item: &StoragePath, changes: &PropertyChanges) -> Result<(), VfsError> {
        self.mounts.resolve(item.mount())?.metadata_editor()?.update(item, changes).await
    }
}

pub struct CreateShareLink {
    mounts: Arc<dyn MountResolver>,
}

impl CreateShareLink {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// `valid_for` is clamped to [`MAX_SHARE_LINK_VALIDITY`].
    ///
    /// # Errors
    /// `Unsupported(Presign)` on providers without share links.
    pub async fn execute(&self, item: &StoragePath, valid_for: Duration) -> Result<String, VfsError> {
        let presigner = self.mounts.resolve(item.mount())?.presigner()?;
        presigner.share_link(item, valid_for.min(MAX_SHARE_LINK_VALIDITY)).await
    }
}

pub struct ListItemVersions {
    mounts: Arc<dyn MountResolver>,
}

impl ListItemVersions {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// # Errors
    /// `Unsupported(ListVersions)` on providers without versioning.
    pub async fn execute(&self, item: &StoragePath) -> Result<Vec<ItemVersion>, VfsError> {
        self.mounts.resolve(item.mount())?.version_lister()?.versions(item).await
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

opaque_debug!(GetItemProperties, UpdateItemProperties, CreateShareLink, ListItemVersions);
