use std::future::Future;

use domain::{EntryBatch, FileEntry, StoragePath, VfsError};
use futures::future::BoxFuture;
use futures::stream::{BoxStream, Stream};
use futures::{FutureExt, StreamExt};

use crate::cancel::CancelToken;

/// Enumerates directories.
///
/// Contract (verified by `vfs-contract-tests`):
/// - batches hold at most [`domain::LIST_BATCH_SIZE`] entries, every entry is a direct child of `dir`;
/// - a missing `dir` yields a single `Err(NotFound)`; a non-directory yields `Err(NotADirectory)`;
/// - once `cancel` fires the stream yields `Err(Cancelled)` and ends;
/// - `stat` of a listed entry returns an equal entry.
pub trait Lister: Send + Sync + 'static {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static;

    fn stat(&self, path: &StoragePath) -> impl Future<Output = Result<FileEntry, VfsError>> + Send;
}

/// Object-safe form of [`Lister`].
pub trait DynLister: Send + Sync {
    fn list_boxed(&self, dir: &StoragePath, cancel: CancelToken) -> BoxStream<'static, Result<EntryBatch, VfsError>>;

    fn stat_boxed<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<FileEntry, VfsError>>;
}

impl<T: Lister> DynLister for T {
    fn list_boxed(&self, dir: &StoragePath, cancel: CancelToken) -> BoxStream<'static, Result<EntryBatch, VfsError>> {
        self.list(dir, cancel).boxed()
    }

    fn stat_boxed<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<FileEntry, VfsError>> {
        self.stat(path).boxed()
    }
}
