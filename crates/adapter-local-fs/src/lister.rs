use std::fs;
use std::path::Path;

use application::CancelToken;
use application::ports::Lister;
use domain::{EntryBatch, FileEntry, LIST_BATCH_SIZE, PathError, PathSegment, StoragePath, VfsError};
use futures::stream::{self, Stream};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::entry::build_entry;
use crate::error::from_io;
use crate::native_path::{non_utf8, to_native};

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
        self.runtime
            .spawn_blocking(move || stat_blocking(&path))
            .await
            .map_err(|join| VfsError::Io { message: join.to_string() })?
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
            // Entries that vanish or have non-UTF-8 names mid-listing are skipped;
            // the watcher reports the former, the latter is a known M1 limitation.
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
    let name = PathSegment::new(dir_entry.file_name().to_str()?).ok()?;
    let metadata = dir_entry.metadata().ok()?;
    Some(build_entry(parent, name, &dir_entry.path(), &metadata))
}

pub(crate) fn stat_blocking(path: &StoragePath) -> Result<FileEntry, VfsError> {
    let native = to_native(path)?;
    let (parent, name) = parent_and_name(path, &native)?;
    let metadata = fs::symlink_metadata(&native).map_err(|e| from_io(&e))?;
    Ok(build_entry(&parent, name, &native, &metadata))
}

fn parent_and_name(path: &StoragePath, native: &Path) -> Result<(StoragePath, PathSegment), VfsError> {
    let parent = path.parent().ok_or(VfsError::InvalidPath(PathError::Root))?;
    let name = path.name().cloned().ok_or_else(|| non_utf8(native))?;
    Ok((parent, name))
}
