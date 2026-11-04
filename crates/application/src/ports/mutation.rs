//! Roles that change a mount. Each is a static trait for adapters plus an
//! object-safe `Dyn*` twin with a blanket impl (ADR 0002).
//!
//! Shared contract (verified by `vfs-contract-tests`):
//! - nothing ever overwrites an existing item; that is `Err(AlreadyExists)`;
//! - a missing source is `Err(NotFound)`, a missing destination folder `Err(NotFound)`;
//! - a failed or cancelled copy leaves no partial destination behind.

use std::future::Future;
use std::sync::Arc;

use domain::{ByteSize, FileEntry, StoragePath, TrashTicket, VfsError};
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::cancel::CancelToken;

/// Reports bytes transferred since the previous call.
pub type ProgressCallback = Arc<dyn Fn(ByteSize) + Send + Sync>;

/// Copies one non-directory item (file or symlink) within a mount.
pub trait Copier: Send + Sync + 'static {
    /// Returns the bytes copied. Directories are `Err(IsADirectory)`; callers
    /// create folders with [`FolderMaker`] and copy their contents item by item.
    fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> impl Future<Output = Result<ByteSize, VfsError>> + Send;
}

/// Renames/moves an item within a mount.
pub trait Mover: Send + Sync + 'static {
    /// `Err(CrossDevice)` when the provider cannot move without copying; the
    /// caller then falls back to copy + remove.
    fn rename(&self, from: &StoragePath, to: &StoragePath) -> impl Future<Output = Result<(), VfsError>> + Send;
}

/// Deletes permanently (recursively for folders). Not undoable.
pub trait Remover: Send + Sync + 'static {
    fn remove(&self, path: &StoragePath, cancel: CancelToken) -> impl Future<Output = Result<(), VfsError>> + Send;
}

pub trait FolderMaker: Send + Sync + 'static {
    fn create_folder(&self, path: &StoragePath) -> impl Future<Output = Result<FileEntry, VfsError>> + Send;
}

/// The platform trash, with exact restore.
pub trait Trasher: Send + Sync + 'static {
    fn trash(&self, path: &StoragePath) -> impl Future<Output = Result<TrashTicket, VfsError>> + Send;

    /// `Err(AlreadyExists)` if the original location is occupied again;
    /// `Err(NotInTrash)` if the item was emptied or removed from the trash.
    fn restore(&self, ticket: &TrashTicket) -> impl Future<Output = Result<(), VfsError>> + Send;
}

/// Opens an item with the system's default application.
pub trait Launcher: Send + Sync + 'static {
    fn launch(&self, path: &StoragePath) -> impl Future<Output = Result<(), VfsError>> + Send;
}

pub trait DynCopier: Send + Sync {
    fn copy_item_boxed<'a>(
        &'a self,
        from: &'a StoragePath,
        to: &'a StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<ByteSize, VfsError>>;
}

impl<T: Copier> DynCopier for T {
    fn copy_item_boxed<'a>(
        &'a self,
        from: &'a StoragePath,
        to: &'a StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<ByteSize, VfsError>> {
        self.copy_item(from, to, progress, cancel).boxed()
    }
}

pub trait DynMover: Send + Sync {
    fn rename_boxed<'a>(&'a self, from: &'a StoragePath, to: &'a StoragePath) -> BoxFuture<'a, Result<(), VfsError>>;
}

impl<T: Mover> DynMover for T {
    fn rename_boxed<'a>(&'a self, from: &'a StoragePath, to: &'a StoragePath) -> BoxFuture<'a, Result<(), VfsError>> {
        self.rename(from, to).boxed()
    }
}

pub trait DynRemover: Send + Sync {
    fn remove_boxed<'a>(&'a self, path: &'a StoragePath, cancel: CancelToken) -> BoxFuture<'a, Result<(), VfsError>>;
}

impl<T: Remover> DynRemover for T {
    fn remove_boxed<'a>(&'a self, path: &'a StoragePath, cancel: CancelToken) -> BoxFuture<'a, Result<(), VfsError>> {
        self.remove(path, cancel).boxed()
    }
}

pub trait DynFolderMaker: Send + Sync {
    fn create_folder_boxed<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<FileEntry, VfsError>>;
}

impl<T: FolderMaker> DynFolderMaker for T {
    fn create_folder_boxed<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<FileEntry, VfsError>> {
        self.create_folder(path).boxed()
    }
}

pub trait DynTrasher: Send + Sync {
    fn trash_boxed<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<TrashTicket, VfsError>>;
    fn restore_boxed<'a>(&'a self, ticket: &'a TrashTicket) -> BoxFuture<'a, Result<(), VfsError>>;
}

impl<T: Trasher> DynTrasher for T {
    fn trash_boxed<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<TrashTicket, VfsError>> {
        self.trash(path).boxed()
    }

    fn restore_boxed<'a>(&'a self, ticket: &'a TrashTicket) -> BoxFuture<'a, Result<(), VfsError>> {
        self.restore(ticket).boxed()
    }
}

pub trait DynLauncher: Send + Sync {
    fn launch_boxed<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<(), VfsError>>;
}

impl<T: Launcher> DynLauncher for T {
    fn launch_boxed<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<(), VfsError>> {
        self.launch(path).boxed()
    }
}
