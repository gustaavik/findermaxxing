use std::fs;
use std::future::Future;
use std::os::unix::ffi::OsStrExt;

use application::CancelToken;
use application::ports::Lister;
use domain::{EntryBatch, FileEntry, LIST_BATCH_SIZE, PathError, PathSegment, StoragePath, VfsError};
use futures::stream::{self, Stream};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::entry::build_entry;
use crate::error::from_io;
use crate::native_path::{segment_of, to_native};

/// Batches in flight between the blocking reader and the consumer. Small, so a
/// slow consumer applies back-pressure instead of buffering a whole 100k listing.
const BATCHES_IN_FLIGHT: usize = 4;

/// Local filesystem provider. Blocking syscalls run on the injected runtime's
/// blocking pool so callers never stall an async worker.
#[derive(Debug, Clone)]
pub struct LocalFs {
    runtime: Handle,
}

impl LocalFs {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }

    pub(crate) fn runtime(&self) -> &Handle {
        &self.runtime
    }

    /// Runs blocking filesystem work on the runtime's blocking pool.
    pub(crate) fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, VfsError> + Send + 'static,
    ) -> impl Future<Output = Result<T, VfsError>> + Send + 'static {
        let task = self.runtime.spawn_blocking(work);
        async move { task.await.map_err(|join| VfsError::Io { message: join.to_string() })? }
    }
}

impl Lister for LocalFs {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let (sender, receiver) = mpsc::channel(BATCHES_IN_FLIGHT);
        let dir = dir.clone();
        self.runtime.spawn_blocking(move || read_dir_in_batches(&dir, &cancel, &sender));
        stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) })
    }

    async fn stat(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        let path = path.clone();
        self.blocking(move || stat_blocking(&path)).await
    }
}

type BatchSender = mpsc::Sender<Result<EntryBatch, VfsError>>;

fn read_dir_in_batches(dir: &StoragePath, cancel: &CancelToken, sender: &BatchSender) {
    let result = to_native(dir).and_then(|native| {
        let entries = fs::read_dir(&native).map_err(|e| from_io(&e))?;
        let mut batch = Vec::with_capacity(LIST_BATCH_SIZE);
        for dir_entry in entries {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            // Entries that vanish mid-listing are skipped; the watcher reports them.
            let Ok(dir_entry) = dir_entry else { continue };
            let Some(entry) = entry_from_dir_entry(dir, &dir_entry) else { continue };
            batch.push(entry);
            if batch.len() == LIST_BATCH_SIZE && sender.blocking_send(Ok(std::mem::take(&mut batch))).is_err() {
                return Ok(());
            }
        }
        if !batch.is_empty() {
            // The consumer may have gone away; nothing left to do either way.
            let _ = sender.blocking_send(Ok(batch));
        }
        Ok(())
    });
    if let Err(error) = result {
        let _ = sender.blocking_send(Err(error));
    }
}

fn entry_from_dir_entry(parent: &StoragePath, dir_entry: &fs::DirEntry) -> Option<FileEntry> {
    let name = PathSegment::from_bytes(dir_entry.file_name().as_bytes()).ok()?;
    let metadata = dir_entry.metadata().ok()?;
    Some(build_entry(parent, name, &dir_entry.path(), &metadata))
}

pub(crate) fn stat_blocking(path: &StoragePath) -> Result<FileEntry, VfsError> {
    let native = to_native(path)?;
    let parent = path.parent().ok_or(VfsError::InvalidPath(PathError::Root))?;
    let name = segment_of(&native).ok_or(VfsError::InvalidPath(PathError::Root))?;
    let metadata = fs::symlink_metadata(&native).map_err(|e| from_io(&e))?;
    Ok(build_entry(&parent, name, &native, &metadata))
}
