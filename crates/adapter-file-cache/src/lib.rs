//! The local cache of remote files (ADR 0020), as decorators around any
//! provider that can be read but has no local files of its own:
//!
//! - [`CachingResolver`] wraps the mount registry. Remote mounts get a
//!   cached `Materializer` and a `Lister` that remembers pinned folders;
//!   a mount that can't be opened at all (offline, not connected) falls back
//!   to a read-only view of what is pinned.
//! - [`FileCache`] owns the cache folder: downloads, freshness checks and
//!   least-recently-used eviction under [`domain::CachePolicy`], never
//!   evicting pinned items.

mod lister;
mod materializer;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use application::ports::{
    AccountMounts, CacheIndex, CacheJanitor, CacheRecord, MountHandle, MountResolver, PinStore, StoreError, is_pinned,
};
use domain::{CachePolicy, CachedCopy, Capabilities, MountId, Retention, StoragePath, Timestamp, VfsError};
use tokio::runtime::Handle;

pub use lister::{CachingLister, SnapshotLister};
pub use materializer::CachedMaterializer;

/// The cache folder plus its index; cheap to clone.
#[derive(Clone)]
pub struct FileCache {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    index: Arc<dyn CacheIndex>,
    pins: Arc<dyn PinStore>,
    policy: CachePolicy,
    runtime: Handle,
    evicting: AtomicBool,
}

impl FileCache {
    #[must_use]
    pub fn new(
        root: PathBuf,
        index: Arc<dyn CacheIndex>,
        pins: Arc<dyn PinStore>,
        policy: CachePolicy,
        runtime: Handle,
    ) -> Self {
        Self { inner: Arc::new(Inner { root, index, pins, policy, runtime, evicting: AtomicBool::new(false) }) }
    }

    pub(crate) fn index(&self) -> &Arc<dyn CacheIndex> {
        &self.inner.index
    }

    pub(crate) fn absolute(&self, relative: &str) -> PathBuf {
        self.inner.root.join(relative)
    }

    /// Whether `location` is pinned or inside a pinned folder. A store
    /// failure counts as "not pinned": caching still works, offline doesn't.
    pub(crate) async fn is_pinned(&self, location: &StoragePath) -> bool {
        self.inner.pins.pins().await.is_ok_and(|pins| is_pinned(&pins, location))
    }

    /// Deletes least recently used, unpinned copies until the cache fits its
    /// limit. Runs in the background, one pass at a time.
    pub(crate) fn evict_in_background(&self) {
        if self.inner.evicting.swap(true, Ordering::AcqRel) {
            return;
        }
        let cache = self.clone();
        self.inner.runtime.spawn(async move {
            let _ = cache.evict().await;
            cache.inner.evicting.store(false, Ordering::Release);
        });
    }

    /// # Errors
    /// The index or pin store can't be read.
    pub async fn evict(&self) -> Result<Vec<StoragePath>, application::ports::StoreError> {
        let records = self.inner.index.all().await?;
        let pins = self.inner.pins.pins().await?;
        let copies: Vec<CachedCopy<CacheRecord>> = records
            .into_iter()
            .map(|record| CachedCopy {
                size: record.size,
                last_used: record.last_used,
                retention: if is_pinned(&pins, &record.location) { Retention::Pinned } else { Retention::Evictable },
                key: record,
            })
            .collect();
        let evicted = self.inner.policy.evictions(&copies);
        for record in &evicted {
            let _ = tokio::fs::remove_file(self.absolute(&record.file)).await;
        }
        let locations: Vec<StoragePath> = evicted.into_iter().map(|record| record.location).collect();
        self.inner.index.remove(locations.clone()).await?;
        Ok(locations)
    }
}

impl CacheJanitor for FileCache {
    fn forget(&self, account: AccountMounts) -> futures::future::BoxFuture<'static, Result<(), StoreError>> {
        let cache = self.clone();
        Box::pin(async move {
            let index = &cache.inner.index;
            let records: Vec<CacheRecord> =
                index.all().await?.into_iter().filter(|r| account.contains(r.location.mount())).collect();
            for record in &records {
                let _ = tokio::fs::remove_file(cache.absolute(&record.file)).await;
            }
            index.remove(records.into_iter().map(|record| record.location).collect()).await?;
            let folders = index.listed_folders().await?.into_iter().filter(|f| account.contains(f.mount())).collect();
            index.remove_listings(folders).await?;
            for pin in cache.inner.pins.pins().await?.into_iter().filter(|p| account.contains(p.mount())) {
                cache.inner.pins.unpin(pin).await?;
            }
            Ok(())
        })
    }
}

impl std::fmt::Debug for FileCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileCache").field("root", &self.inner.root).finish_non_exhaustive()
    }
}

pub(crate) fn now() -> Timestamp {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis());
    Timestamp::from_unix_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// Errors that mean "can't reach the provider right now", as opposed to an
/// answer from it (`NotFound`, `PermissionDenied`, …).
pub(crate) fn is_unreachable(error: &VfsError) -> bool {
    matches!(error, VfsError::Transient { .. } | VfsError::Io { .. })
}

/// Where a location's copy lives below the cache root:
/// `<scheme>/<authority>/<segments…>`, names kept byte for byte so opened
/// copies keep their real names and extensions.
pub(crate) fn relative_file(location: &StoragePath) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let mount = location.mount();
    let mut path = PathBuf::from(mount.scheme().as_str()).join(sanitized_authority(mount.authority()));
    for segment in location.segments() {
        path.push(std::ffi::OsStr::from_bytes(segment.as_bytes()));
    }
    path
}

/// Authorities (`3@photos`, `12`) never contain separators, but stay safe.
fn sanitized_authority(authority: &str) -> String {
    let cleaned: String = authority.chars().map(|c| if c == '/' || c == '\0' { '_' } else { c }).collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." { format!("_{cleaned}") } else { cleaned }
}

/// Adds caching to remote mounts and an offline view to unreachable ones.
pub struct CachingResolver {
    inner: Arc<dyn MountResolver>,
    cache: FileCache,
}

impl CachingResolver {
    #[must_use]
    pub fn new(inner: Arc<dyn MountResolver>, cache: FileCache) -> Self {
        Self { inner, cache }
    }
}

impl MountResolver for CachingResolver {
    fn resolve(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        match self.inner.resolve(mount) {
            Ok(handle) => Ok(self.decorate(handle)),
            // Nobody can serve this scheme: nothing was ever cached for it.
            Err(error @ VfsError::UnknownScheme(_)) => Err(error),
            Err(error) => Ok(self.offline(error)),
        }
    }
}

impl CachingResolver {
    /// Mounts with their own local files (the local disk) are left alone.
    fn decorate(&self, handle: MountHandle) -> MountHandle {
        let (Ok(lister), Ok(reader), Err(_)) = (handle.lister(), handle.reader(), handle.materializer()) else {
            return handle;
        };
        let caching_lister = Arc::new(CachingLister::new(lister.clone(), self.cache.clone()));
        let materializer = Arc::new(CachedMaterializer::online(caching_lister.clone(), reader, self.cache.clone()));
        handle.into_builder().lister(caching_lister).materializer(materializer).build()
    }

    /// Read-only: browse pinned folders and open their copies.
    fn offline(&self, error: VfsError) -> MountHandle {
        let lister = Arc::new(SnapshotLister::new(self.cache.clone(), error.clone()));
        let materializer = Arc::new(CachedMaterializer::offline(lister.clone(), self.cache.clone(), error));
        MountHandle::builder(Capabilities::empty()).lister(lister).materializer(materializer).build()
    }
}

impl std::fmt::Debug for CachingResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachingResolver").field("cache", &self.cache).finish_non_exhaustive()
    }
}

pub(crate) fn exists(path: &Path) -> bool {
    path.is_file()
}

macro_rules! opaque_debug {
    ($($name:ident),+) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )+};
}

opaque_debug!(CachingLister, SnapshotLister, CachedMaterializer);
