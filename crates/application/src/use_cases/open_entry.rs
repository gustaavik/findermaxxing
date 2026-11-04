use std::sync::Arc;

use domain::{StoragePath, VfsError};

use crate::ports::MountResolver;

/// Opens an item with the system's default application.
pub struct OpenEntry {
    mounts: Arc<dyn MountResolver>,
}

impl OpenEntry {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// # Errors
    /// `Unsupported(Launch)` for providers that cannot open items (remote ones until M4).
    pub async fn execute(&self, item: &StoragePath) -> Result<(), VfsError> {
        let launcher = self.mounts.resolve(item.mount())?.launcher()?;
        launcher.launch_boxed(item).await
    }
}

impl std::fmt::Debug for OpenEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenEntry").finish_non_exhaustive()
    }
}
