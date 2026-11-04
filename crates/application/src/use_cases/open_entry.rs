use std::sync::Arc;

use domain::{Capability, StoragePath, VfsError};

use crate::ports::{LocalFileOpener, MountResolver};

/// Opens an item with the system's default application. Items on providers
/// that can't launch (S3) are first materialised as a local file.
pub struct OpenEntry {
    mounts: Arc<dyn MountResolver>,
    opener: Arc<dyn LocalFileOpener>,
}

impl OpenEntry {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, opener: Arc<dyn LocalFileOpener>) -> Self {
        Self { mounts, opener }
    }

    /// # Errors
    /// `Unsupported(Launch)` when the provider can neither launch nor materialise.
    pub async fn execute(&self, item: &StoragePath) -> Result<(), VfsError> {
        let mount = self.mounts.resolve(item.mount())?;
        if let Ok(launcher) = mount.launcher() {
            return launcher.launch_boxed(item).await;
        }
        let materializer = mount.materializer().map_err(|_| VfsError::Unsupported(Capability::Launch))?;
        let local = materializer.local_file_boxed(item).await?;
        self.opener.open(&local).await
    }
}

impl std::fmt::Debug for OpenEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenEntry").finish_non_exhaustive()
    }
}
