//! In-memory fakes of the ports. They pass the same contract suite as real
//! adapters (see `vfs-contract-tests`), which keeps use-case tests honest.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use domain::{
    ByteSize, Capabilities, ChangeEvent, EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, MountId,
    Scheme, StoragePath, VfsError, Visibility,
};
use futures::FutureExt;
use futures::channel::mpsc;
use futures::future::{self, BoxFuture};
use futures::stream::{self, Stream};

use crate::cancel::CancelToken;
use crate::ports::{Clock, Lister, MountHandle, ProviderFactory, Watcher};

pub const FAKE_SCHEME: &str = "fake";
const HIDDEN_PREFIX: char = '.';

#[derive(Debug, Clone)]
struct Node {
    kind: EntryKind,
    metadata: EntryMetadata,
}

#[derive(Debug, Default)]
struct State {
    nodes: BTreeMap<StoragePath, Node>,
    watchers: Vec<(StoragePath, mpsc::UnboundedSender<ChangeEvent>)>,
}

/// An in-memory filesystem under the `fake://` scheme. Clones share state.
#[derive(Debug, Clone)]
pub struct FakeVfs {
    state: Arc<Mutex<State>>,
}

impl Default for FakeVfs {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeVfs {
    #[must_use]
    pub fn new() -> Self {
        let fake = Self { state: Arc::default() };
        let root = fake.path("/");
        fake.lock().nodes.insert(root, Node { kind: EntryKind::Directory, metadata: EntryMetadata::default() });
        fake
    }

    /// `fake://` + `absolute`. Invalid input is a bug in the test itself.
    #[must_use]
    pub fn path(&self, absolute: &str) -> StoragePath {
        match StoragePath::parse(&format!("{FAKE_SCHEME}://{absolute}")) {
            Ok(path) => path,
            Err(error) => unreachable_in_tests(&error),
        }
    }

    pub fn add_dir(&self, absolute: &str) {
        self.insert(absolute, EntryKind::Directory, None);
    }

    pub fn add_file(&self, absolute: &str, size: u64) {
        self.insert(absolute, EntryKind::File, Some(ByteSize(size)));
    }

    pub fn remove(&self, absolute: &str) {
        let path = self.path(absolute);
        let mut state = self.lock();
        state.nodes.remove(&path);
        notify(&mut state, &path, &ChangeEvent::Removed(path.clone()));
    }

    pub fn emit_rescan(&self) {
        let mut state = self.lock();
        state.watchers.retain(|(_, sender)| sender.unbounded_send(ChangeEvent::Rescan).is_ok());
    }

    /// The `Upserted` event the fake emits for `absolute`.
    #[must_use]
    pub fn upserted(&self, absolute: &str) -> ChangeEvent {
        let path = self.path(absolute);
        match self.lock().nodes.get(&path).and_then(|node| to_entry(&path, node)) {
            Some(entry) => ChangeEvent::Upserted(entry),
            None => ChangeEvent::Removed(path),
        }
    }

    #[must_use]
    pub fn into_factory(self) -> Arc<dyn ProviderFactory> {
        Arc::new(FakeFactory { fake: self })
    }

    fn insert(&self, absolute: &str, kind: EntryKind, size: Option<ByteSize>) {
        let path = self.path(absolute);
        let hidden = path.name().is_some_and(|name| name.as_str().starts_with(HIDDEN_PREFIX));
        let visibility = if hidden { Visibility::Hidden } else { Visibility::Visible };
        let node = Node { kind, metadata: EntryMetadata { size, visibility, ..EntryMetadata::default() } };
        let mut state = self.lock();
        let event = to_entry(&path, &node).map(ChangeEvent::Upserted);
        state.nodes.insert(path.clone(), node);
        if let Some(event) = event {
            notify(&mut state, &path, &event);
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn require_dir(&self, dir: &StoragePath) -> Result<(), VfsError> {
        match self.lock().nodes.get(dir) {
            None => Err(VfsError::NotFound),
            Some(node) if node.kind.is_container() => Ok(()),
            Some(_) => Err(VfsError::NotADirectory),
        }
    }

    fn children(&self, dir: &StoragePath) -> Vec<FileEntry> {
        self.lock()
            .nodes
            .iter()
            .filter(|(path, _)| path.is_child_of(dir))
            .filter_map(|(path, node)| to_entry(path, node))
            .collect()
    }
}

fn to_entry(path: &StoragePath, node: &Node) -> Option<FileEntry> {
    Some(FileEntry::new(&path.parent()?, path.name()?.clone(), node.kind, node.metadata.clone()))
}

fn notify(state: &mut State, changed: &StoragePath, event: &ChangeEvent) {
    state.watchers.retain(|(dir, sender)| !changed.is_child_of(dir) || sender.unbounded_send(event.clone()).is_ok());
}

impl Lister for FakeVfs {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let batches: Result<Vec<EntryBatch>, VfsError> = self
            .require_dir(dir)
            .map(|()| self.children(dir).chunks(LIST_BATCH_SIZE).map(<[FileEntry]>::to_vec).collect());
        cancellable(batches, cancel)
    }

    async fn stat(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        let state = self.lock();
        let node = state.nodes.get(path).ok_or(VfsError::NotFound)?;
        to_entry(path, node).ok_or(VfsError::NotFound)
    }
}

/// Yields the batches, or `Err(Cancelled)` and stops as soon as `cancel` fires.
fn cancellable(
    batches: Result<Vec<EntryBatch>, VfsError>,
    cancel: CancelToken,
) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
    let items: Vec<Result<EntryBatch, VfsError>> = match batches {
        Ok(batches) => batches.into_iter().map(Ok).collect(),
        Err(error) => vec![Err(error)],
    };
    stream::unfold((items.into_iter(), cancel, false), |(mut items, cancel, finished)| async move {
        if finished {
            return None;
        }
        if cancel.is_cancelled() {
            return Some((Err(VfsError::Cancelled), (items, cancel, true)));
        }
        items.next().map(|item| (item, (items, cancel, false)))
    })
}

impl Watcher for FakeVfs {
    fn watch(&self, dir: &StoragePath) -> Result<impl Stream<Item = ChangeEvent> + Send + 'static, VfsError> {
        self.require_dir(dir)?;
        let (sender, receiver) = mpsc::unbounded();
        self.lock().watchers.push((dir.clone(), sender));
        Ok(receiver)
    }
}

#[derive(Debug)]
struct FakeFactory {
    fake: FakeVfs,
}

impl ProviderFactory for FakeFactory {
    fn scheme(&self) -> Scheme {
        match Scheme::new(FAKE_SCHEME) {
            Ok(scheme) => scheme,
            Err(error) => unreachable_in_tests(&error),
        }
    }

    fn open(&self, _mount: &MountId) -> Result<MountHandle, VfsError> {
        let fake = Arc::new(self.fake.clone());
        Ok(MountHandle::builder(Capabilities::empty()).lister(fake.clone()).watcher(fake).build())
    }
}

/// A clock whose sleeps complete immediately.
#[derive(Debug, Clone, Copy, Default)]
pub struct InstantClock;

impl Clock for InstantClock {
    fn sleep(&self, _duration: Duration) -> BoxFuture<'static, ()> {
        future::ready(()).boxed()
    }
}

#[allow(clippy::panic, reason = "test-support code: invalid fixtures are programmer errors")]
fn unreachable_in_tests(error: &dyn std::error::Error) -> ! {
    panic!("invalid test fixture: {error}")
}
