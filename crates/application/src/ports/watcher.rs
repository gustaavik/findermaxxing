use domain::{ChangeEvent, StoragePath, VfsError};
use futures::StreamExt;
use futures::stream::{BoxStream, Stream};

/// Observes direct children of a directory. Dropping the stream stops watching.
///
/// Contract: creating or removing a child is reported within a bounded delay;
/// if events may have been lost the stream yields [`ChangeEvent::Rescan`].
pub trait Watcher: Send + Sync + 'static {
    /// # Errors
    /// `NotFound` / `NotADirectory` / `PermissionDenied` when `dir` cannot be watched.
    fn watch(&self, dir: &StoragePath) -> Result<impl Stream<Item = ChangeEvent> + Send + 'static, VfsError>;
}

/// Object-safe form of [`Watcher`].
pub trait DynWatcher: Send + Sync {
    /// # Errors
    /// See [`Watcher::watch`].
    fn watch_boxed(&self, dir: &StoragePath) -> Result<BoxStream<'static, ChangeEvent>, VfsError>;
}

impl<T: Watcher> DynWatcher for T {
    fn watch_boxed(&self, dir: &StoragePath) -> Result<BoxStream<'static, ChangeEvent>, VfsError> {
        self.watch(dir).map(StreamExt::boxed)
    }
}
