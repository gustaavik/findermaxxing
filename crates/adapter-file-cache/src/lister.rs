//! Listing that remembers pinned folders, and the offline listing that
//! replays what was remembered.

use std::sync::{Arc, Mutex, PoisonError};

use application::CancelToken;
use application::ports::{DynLister, Lister};
use domain::{EntryBatch, FileEntry, LIST_BATCH_SIZE, StoragePath, VfsError};
use futures::StreamExt;
use futures::stream::{self, BoxStream, Stream};

use crate::{FileCache, is_unreachable};

/// Wraps a provider's lister: pinned folders' listings are stored as they
/// stream past, and replayed when the provider can't be reached.
pub struct CachingLister {
    inner: Arc<dyn DynLister>,
    cache: FileCache,
}

impl CachingLister {
    #[must_use]
    pub fn new(inner: Arc<dyn DynLister>, cache: FileCache) -> Self {
        Self { inner, cache }
    }
}

impl Lister for CachingLister {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let (inner, cache, dir) = (Arc::clone(&self.inner), self.cache.clone(), dir.clone());
        stream::once(async move {
            let mut upstream = inner.list_boxed(&dir, cancel).fuse();
            let first = upstream.next().await;
            if let Some(Err(error)) = &first
                && is_unreachable(error)
                && let Some(snapshot) = snapshot(&cache, &dir).await
            {
                return snapshot;
            }
            let replayed = stream::iter(first).chain(upstream).boxed();
            if cache.is_pinned(&dir).await { remembering(replayed, cache, dir) } else { replayed }
        })
        .flatten()
    }

    async fn stat(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        match self.inner.stat_boxed(path).await {
            Err(error) if is_unreachable(&error) => remembered_entry(&self.cache, path).await.ok_or(error),
            other => other,
        }
    }
}

/// Passes batches through and stores the complete listing at the end. A
/// listing that fails part-way is not stored, so a snapshot is never partial.
fn remembering(
    upstream: BoxStream<'static, Result<EntryBatch, VfsError>>,
    cache: FileCache,
    dir: StoragePath,
) -> BoxStream<'static, Result<EntryBatch, VfsError>> {
    let seen = Arc::new(Mutex::new(Some(Vec::new())));
    let collecting = Arc::clone(&seen);
    let batches = upstream.inspect(move |batch| {
        let mut seen = collecting.lock().unwrap_or_else(PoisonError::into_inner);
        match batch {
            Ok(entries) => {
                if let Some(all) = seen.as_mut() {
                    all.extend(entries.iter().cloned());
                }
            }
            Err(_) => *seen = None,
        }
    });
    let store = stream::once(async move {
        let complete = seen.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(entries) = complete {
            let _ = cache.index().save_listing(&dir, entries).await;
        }
    })
    .filter_map(|()| futures::future::ready(None));
    batches.chain(store).boxed()
}

async fn snapshot(cache: &FileCache, dir: &StoragePath) -> Option<BoxStream<'static, Result<EntryBatch, VfsError>>> {
    let entries = cache.index().listing(dir).await.ok()??;
    let batches: Vec<Result<EntryBatch, VfsError>> =
        entries.chunks(LIST_BATCH_SIZE).map(|chunk| Ok(chunk.to_vec())).collect();
    Some(stream::iter(batches).boxed())
}

/// An entry from its parent's stored listing.
async fn remembered_entry(cache: &FileCache, path: &StoragePath) -> Option<FileEntry> {
    let entries = cache.index().listing(&path.parent()?).await.ok()??;
    entries.into_iter().find(|entry| entry.path() == path)
}

/// The offline view of a mount that can't be opened: pinned folders from
/// their stored listings, everything else fails with the original error.
pub struct SnapshotLister {
    cache: FileCache,
    error: VfsError,
}

impl SnapshotLister {
    #[must_use]
    pub fn new(cache: FileCache, error: VfsError) -> Self {
        Self { cache, error }
    }
}

impl Lister for SnapshotLister {
    fn list(
        &self,
        dir: &StoragePath,
        _cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let (cache, dir, error) = (self.cache.clone(), dir.clone(), self.error.clone());
        stream::once(async move {
            match snapshot(&cache, &dir).await {
                Some(batches) => batches,
                None => stream::iter([Err(error)]).boxed(),
            }
        })
        .flatten()
    }

    async fn stat(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        remembered_entry(&self.cache, path).await.ok_or_else(|| self.error.clone())
    }
}
