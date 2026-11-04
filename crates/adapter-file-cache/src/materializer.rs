//! Local copies of remote files: reused while the remote item is unchanged,
//! downloaded otherwise, and served from the cache when the provider can't
//! be reached.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use application::CancelToken;
use application::ports::{CacheRecord, DynLister, DynReader, LocalFile, Materializer};
use domain::{ByteSize, EntryKind, FileEntry, StoragePath, VfsError};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::{FileCache, exists, is_unreachable, now, relative_file};

const DOWNLOAD_SUFFIX: &str = "findermaxxing-download";

/// Distinguishes concurrent downloads of the same item.
static DOWNLOADS: AtomicU64 = AtomicU64::new(0);

pub struct CachedMaterializer {
    lister: Arc<dyn DynLister>,
    source: Source,
    cache: FileCache,
}

enum Source {
    Provider(Arc<dyn DynReader>),
    /// The provider can't be reached: only existing copies are served, and
    /// anything else fails with why the provider couldn't be opened.
    Unreachable(VfsError),
}

impl CachedMaterializer {
    #[must_use]
    pub fn online(lister: Arc<dyn DynLister>, reader: Arc<dyn DynReader>, cache: FileCache) -> Self {
        Self { lister, source: Source::Provider(reader), cache }
    }

    #[must_use]
    pub fn offline(lister: Arc<dyn DynLister>, cache: FileCache, error: VfsError) -> Self {
        Self { lister, source: Source::Unreachable(error), cache }
    }

    /// The cached copy, if it exists on disk.
    async fn cached(&self, item: &StoragePath) -> Option<(CacheRecord, PathBuf)> {
        let record = self.cache.index().lookup(item).await.ok()??;
        let path = self.cache.absolute(&record.file);
        exists(&path).then_some((record, path))
    }

    async fn serve(&self, record: &CacheRecord, path: PathBuf) -> LocalFile {
        let _ = self.cache.index().touch(&record.location, now()).await;
        LocalFile::new(path)
    }

    async fn download(&self, entry: &FileEntry, reader: &dyn DynReader) -> Result<LocalFile, VfsError> {
        let relative = relative_file(entry.path());
        let target = self.cache.absolute(&relative.to_string_lossy());
        let mut chunks = reader.read_boxed(entry.path(), CancelToken::never()).await?;
        let written = write_atomically(&target, &mut chunks).await?;
        let record = CacheRecord {
            location: entry.path().clone(),
            file: relative.to_string_lossy().into_owned(),
            size: written,
            remote_modified: entry.metadata().modified,
            last_used: now(),
        };
        let _ = self.cache.index().record(record).await;
        self.cache.evict_in_background();
        Ok(LocalFile::new(target))
    }
}

impl Materializer for CachedMaterializer {
    async fn local_file(&self, item: &StoragePath) -> Result<LocalFile, VfsError> {
        let reader = match &self.source {
            Source::Provider(reader) => reader,
            Source::Unreachable(error) => return self.cached_or(item, error.clone()).await,
        };
        let entry = match self.lister.stat_boxed(item).await {
            Ok(entry) => entry,
            Err(error) if is_unreachable(&error) => return self.cached_or(item, error).await,
            Err(error) => return Err(error),
        };
        if entry.kind() == EntryKind::Directory {
            return Err(VfsError::IsADirectory);
        }
        if let Some((record, path)) = self.cached(item).await
            && is_fresh(&record, &entry, &path)
        {
            return Ok(self.serve(&record, path).await);
        }
        self.download(&entry, reader.as_ref()).await
    }
}

impl CachedMaterializer {
    /// Without the provider: whatever copy exists, however old.
    async fn cached_or(&self, item: &StoragePath, error: VfsError) -> Result<LocalFile, VfsError> {
        match self.cached(item).await {
            Some((record, path)) => Ok(self.serve(&record, path).await),
            None => Err(error),
        }
    }
}

/// The copy matches the remote item's size and modification time.
fn is_fresh(record: &CacheRecord, entry: &FileEntry, path: &Path) -> bool {
    let on_disk = std::fs::metadata(path).map(|m| ByteSize(m.len())).ok();
    Some(record.size) == entry.metadata().size
        && on_disk == Some(record.size)
        && record.remote_modified == entry.metadata().modified
}

/// Downloads next to `target` and renames into place, so a reader never sees
/// a half-written copy. Returns the size written.
async fn write_atomically(target: &Path, chunks: &mut application::ports::ByteStream) -> Result<ByteSize, VfsError> {
    let io = |e: std::io::Error| VfsError::Io { message: e.to_string() };
    let folder = target.parent().ok_or(VfsError::NotFound)?;
    make_folder(folder).await.map_err(io)?;
    let serial = DOWNLOADS.fetch_add(1, Ordering::Relaxed);
    let partial = folder.join(format!(
        ".{}.{DOWNLOAD_SUFFIX}-{}-{serial}",
        target.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    let result = async {
        let mut file = tokio::fs::File::create(&partial).await.map_err(io)?;
        let mut written = 0_u64;
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk?;
            written += chunk.len() as u64;
            file.write_all(&chunk).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        tokio::fs::rename(&partial, target).await.map_err(io)?;
        Ok(ByteSize(written))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
    }
    result
}

/// Creates `folder`, replacing a stale cached *file* in its way (the remote
/// item became a folder since it was cached).
async fn make_folder(folder: &Path) -> std::io::Result<()> {
    match tokio::fs::create_dir_all(folder).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let mut ancestor = Some(folder);
            while let Some(path) = ancestor {
                if path.is_file() {
                    tokio::fs::remove_file(path).await?;
                    return tokio::fs::create_dir_all(folder).await;
                }
                ancestor = path.parent();
            }
            Err(error)
        }
    }
}
