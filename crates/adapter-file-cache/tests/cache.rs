//! The cache decorators over a fake remote provider that has no local files
//! of its own and can be taken offline.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adapter_file_cache::{CachingResolver, FileCache};
use application::ports::{CacheIndex, MountHandle, MountResolver, PinStore, ProviderFactory};
use application::testing::{FakeVfs, InMemoryCacheIndex, InMemoryPins};
use application::{CancelToken, ProviderRegistry};
use domain::{ByteSize, CachePolicy, Capabilities, MountId, Scheme, StoragePath, VfsError};
use futures::TryStreamExt;

/// Only the roles a remote provider has: no materializer of its own.
#[derive(Debug)]
struct Remote {
    fake: FakeVfs,
    scheme: &'static str,
    offline: Arc<AtomicBool>,
}

impl ProviderFactory for Remote {
    fn scheme(&self) -> Scheme {
        Scheme::new(self.scheme).unwrap()
    }

    fn open(&self, _mount: &MountId) -> Result<MountHandle, VfsError> {
        if self.offline.load(Ordering::SeqCst) {
            return Err(VfsError::Transient { retry_after: None });
        }
        let fake = Arc::new(self.fake.clone());
        Ok(MountHandle::builder(Capabilities::empty()).lister(fake.clone()).reader(fake).build())
    }
}

struct World {
    remote: FakeVfs,
    offline: Arc<AtomicBool>,
    index: Arc<InMemoryCacheIndex>,
    pins: Arc<InMemoryPins>,
    cache: FileCache,
    mounts: CachingResolver,
    _dir: tempfile::TempDir,
}

fn world(limit: u64) -> World {
    world_on("remote", "/", limit)
}

/// `root` is the mount's root as the fake spells it (`"/"`, or `"4/"` for a
/// mount with authority 4).
fn world_on(scheme: &'static str, root: &str, limit: u64) -> World {
    let remote = FakeVfs::remote(scheme, None);
    remote.add_dir(root);
    remote.add_dir(&format!("{root}trip"));
    let offline = Arc::new(AtomicBool::new(false));
    let factory = Arc::new(Remote { fake: remote.clone(), scheme, offline: offline.clone() });
    let registry: Arc<dyn MountResolver> = Arc::new(ProviderRegistry::default().with_provider(factory).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let (index, pins) = (Arc::new(InMemoryCacheIndex::default()), Arc::new(InMemoryPins::default()));
    let cache = FileCache::new(
        dir.path().to_path_buf(),
        index.clone(),
        pins.clone(),
        CachePolicy { limit: ByteSize(limit) },
        tokio::runtime::Handle::current(),
    );
    let mounts = CachingResolver::new(registry, cache.clone());
    World { remote, offline, index, pins, cache, mounts, _dir: dir }
}

impl World {
    async fn local_copy(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let location = self.remote.path(path);
        let handle = self.mounts.resolve(location.mount())?;
        let file = handle.materializer()?.local_file_boxed(&location).await?;
        Ok(std::fs::read(file.path()).unwrap())
    }

    async fn names(&self, dir: &str) -> Result<Vec<String>, VfsError> {
        let location = self.remote.path(dir);
        let handle = self.mounts.resolve(location.mount())?;
        let batches: Vec<_> = handle.lister()?.list_boxed(&location, CancelToken::never()).try_collect().await?;
        let mut names: Vec<String> =
            batches.into_iter().flatten().map(|entry| entry.name().display_name().into_owned()).collect();
        names.sort();
        Ok(names)
    }

    async fn cache_file(&self, path: &str) -> std::path::PathBuf {
        let record = self.index.lookup(&self.remote.path(path)).await.unwrap().unwrap();
        self._dir.path().join(record.file)
    }

    async fn cached(&self) -> Vec<StoragePath> {
        let mut locations: Vec<StoragePath> = self.index.all().await.unwrap().into_iter().map(|r| r.location).collect();
        locations.sort();
        locations
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn copies_are_reused_until_the_remote_item_changes() {
    let w = world(1_000_000);
    w.remote.add_file_with("/trip/notes.txt", b"first");
    assert_eq!(w.local_copy("/trip/notes.txt").await.unwrap(), b"first");
    w.remote.fail_on("/trip/notes.txt", VfsError::PermissionDenied);
    assert_eq!(w.local_copy("/trip/notes.txt").await.unwrap(), b"first", "served without reading again");

    let w = world(1_000_000);
    w.remote.add_file_with("/trip/notes.txt", b"first");
    w.local_copy("/trip/notes.txt").await.unwrap();
    w.remote.remove("/trip/notes.txt");
    w.remote.add_file_with("/trip/notes.txt", b"second version");
    assert_eq!(w.local_copy("/trip/notes.txt").await.unwrap(), b"second version");
}

#[tokio::test(flavor = "multi_thread")]
async fn least_recently_used_unpinned_copies_are_evicted() {
    let w = world(25);
    w.remote.add_dir("/keep");
    w.remote.add_file_with("/keep/pinned.bin", &[1; 10]);
    w.remote.add_file_with("/trip/old.bin", &[2; 10]);
    w.remote.add_file_with("/trip/new.bin", &[3; 10]);
    w.pins.pin(w.remote.path("/keep")).await.unwrap();

    w.local_copy("/keep/pinned.bin").await.unwrap();
    w.local_copy("/trip/old.bin").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    w.local_copy("/trip/new.bin").await.unwrap();
    let evicted = w.cache.evict().await.unwrap();

    assert_eq!(evicted, vec![w.remote.path("/trip/old.bin")]);
    assert_eq!(w.cached().await, vec![w.remote.path("/keep/pinned.bin"), w.remote.path("/trip/new.bin")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn pinned_folders_stay_browsable_and_openable_offline() {
    let w = world(1_000_000);
    w.remote.add_file_with("/trip/a.jpg", b"aaaa");
    w.remote.add_file_with("/trip/b.jpg", b"bbbb");
    w.pins.pin(w.remote.path("/trip")).await.unwrap();
    assert_eq!(w.names("/trip").await.unwrap(), ["a.jpg", "b.jpg"]);
    w.local_copy("/trip/a.jpg").await.unwrap();

    w.offline.store(true, Ordering::SeqCst);

    assert_eq!(w.names("/trip").await.unwrap(), ["a.jpg", "b.jpg"], "the remembered listing");
    assert_eq!(w.local_copy("/trip/a.jpg").await.unwrap(), b"aaaa");
    assert_eq!(w.local_copy("/trip/b.jpg").await, Err(VfsError::Transient { retry_after: None }), "never downloaded");
    assert_eq!(w.names("/").await, Err(VfsError::Transient { retry_after: None }), "not pinned");
    let handle = w.mounts.resolve(w.remote.path("/").mount()).unwrap();
    assert!(handle.writer().is_err(), "offline views are read-only");
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_listings_fall_back_to_the_snapshot() {
    let w = world(1_000_000);
    w.remote.add_file_with("/trip/a.jpg", b"aaaa");
    w.pins.pin(w.remote.path("/trip")).await.unwrap();
    w.names("/trip").await.unwrap();

    w.remote.fail_listing("/trip", VfsError::Io { message: "connection reset".into() });
    w.remote.remove("/trip/a.jpg");

    assert_eq!(w.names("/trip").await.unwrap(), ["a.jpg"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn forgetting_an_account_deletes_its_copies_listings_and_pins() {
    use application::ports::{AccountMounts, CacheJanitor};
    use domain::{AccountId, ServerLocation, ServerProtocol};
    let w = world_on("sftp", "4/", 1_000_000);
    w.remote.add_file_with("4/trip/a.jpg", b"aaaa");
    w.pins.pin(w.remote.path("4/trip")).await.unwrap();
    assert_eq!(w.names("4/trip").await.unwrap(), ["a.jpg"]);
    w.local_copy("4/trip/a.jpg").await.unwrap();
    let copy = w.cache_file("4/trip/a.jpg").await;
    assert!(copy.exists());

    let other = ServerLocation { protocol: ServerProtocol::Sftp, account: AccountId(5) };
    w.cache.forget(AccountMounts::Server(other)).await.unwrap();
    assert_eq!(w.cached().await.len(), 1, "another account's copies are kept");

    let mine = ServerLocation { protocol: ServerProtocol::Sftp, account: AccountId(4) };
    w.cache.forget(AccountMounts::Server(mine)).await.unwrap();
    assert!(w.cached().await.is_empty());
    assert!(!copy.exists(), "the copy is deleted from disk");
    assert!(w.pins.pins().await.unwrap().is_empty());
    assert_eq!(w.index.listing(&w.remote.path("4/trip")).await.unwrap(), None);
}
