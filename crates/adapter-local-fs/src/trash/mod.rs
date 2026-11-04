//! The platform trash for local files (ADR 0008).
//!
//! macOS asks Finder's own `NSFileManager` to trash, which also handles
//! external volumes and reports exactly where the item went. Linux and other
//! freedesktop systems follow the Trash specification. Restoring is the same
//! everywhere: a no-replace rename back to the original location.

mod freedesktop;
#[cfg(target_os = "macos")]
mod macos;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use application::ports::Trasher;
use domain::{StoragePath, TrashTicket, VfsError};
use tokio::runtime::Handle;

pub use freedesktop::FreedesktopTrash;

use crate::error::from_io;
use crate::lister::LocalFs;
use crate::native_path::{storage_path_from_native, to_native};
use crate::sys::rename_no_replace;

#[derive(Debug, Clone)]
enum Backend {
    #[cfg(target_os = "macos")]
    MacOs,
    Freedesktop(Arc<FreedesktopTrash>),
}

/// `Trasher` for `file://` items.
#[derive(Debug, Clone)]
pub struct LocalTrash {
    fs: LocalFs,
    backend: Backend,
}

impl LocalTrash {
    /// The trash the user's desktop shows.
    #[must_use]
    pub fn platform_default(runtime: Handle) -> Self {
        #[cfg(target_os = "macos")]
        let backend = Backend::MacOs;
        #[cfg(not(target_os = "macos"))]
        let backend = Backend::Freedesktop(Arc::new(FreedesktopTrash::from_environment()));
        Self { fs: LocalFs::new(runtime), backend }
    }

    /// A freedesktop trash with an explicit home trash directory (tests, sandboxes).
    #[must_use]
    pub fn freedesktop(runtime: Handle, trash: FreedesktopTrash) -> Self {
        Self { fs: LocalFs::new(runtime), backend: Backend::Freedesktop(Arc::new(trash)) }
    }
}

impl Backend {
    fn trash(&self, native: &Path) -> Result<std::path::PathBuf, VfsError> {
        match self {
            #[cfg(target_os = "macos")]
            Self::MacOs => macos::trash(native),
            Self::Freedesktop(trash) => trash.trash(native).map_err(|e| from_io(&e)),
        }
    }

    fn restore(&self, trashed: &Path, original: &Path) -> Result<(), VfsError> {
        if fs::symlink_metadata(trashed).is_err() {
            return Err(VfsError::NotInTrash);
        }
        rename_no_replace(trashed, original).map_err(|e| from_io(&e))?;
        if let Self::Freedesktop(trash) = self {
            trash.forget(trashed);
        }
        Ok(())
    }
}

impl Trasher for LocalTrash {
    async fn trash(&self, path: &StoragePath) -> Result<TrashTicket, VfsError> {
        let (native, original) = (to_native(path)?, path.clone());
        let backend = self.backend.clone();
        self.fs
            .blocking(move || {
                let trashed = backend.trash(&native)?;
                Ok(TrashTicket { original, trashed: storage_path_from_native(&trashed)? })
            })
            .await
    }

    async fn restore(&self, ticket: &TrashTicket) -> Result<(), VfsError> {
        let (trashed, original) = (to_native(&ticket.trashed)?, to_native(&ticket.original)?);
        let backend = self.backend.clone();
        self.fs.blocking(move || backend.restore(&trashed, &original)).await
    }
}
