//! In-memory fakes of the ports. They pass the same contract suite as real
//! adapters (see `vfs-contract-tests`), which keeps use-case tests honest.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use domain::{
    ByteSize, Capabilities, Capability, ChangeEvent, Conflict, ConflictResolution, EntryBatch, EntryKind,
    EntryMetadata, FileEntry, JobProgress, LIST_BATCH_SIZE, MountId, PathSegment, Scheme, StoragePath, TrashTicket,
    VfsError, Visibility,
};
use futures::FutureExt;
use futures::channel::mpsc;
use futures::future::{self, BoxFuture};
use futures::stream::{self, Stream};

use crate::cancel::CancelToken;
use crate::ports::{
    Clock, ConflictResolver, Copier, FolderMaker, JobObserver, Launcher, Lister, MountHandle, Mover, ProgressCallback,
    ProviderFactory, Remover, Trasher, Watcher,
};

pub const FAKE_SCHEME: &str = "fake";
const HIDDEN_PREFIX: u8 = b'.';
const TRASH_ROOT: &str = "/.fake-trash";

#[derive(Debug, Clone)]
struct Node {
    kind: EntryKind,
    metadata: EntryMetadata,
    content: Vec<u8>,
}

#[derive(Debug, Default)]
struct State {
    nodes: BTreeMap<StoragePath, Node>,
    /// Trashed subtrees keyed by their trash location, nodes keyed by original path.
    trash: HashMap<StoragePath, Vec<(StoragePath, Node)>>,
    trashed_count: u64,
    watchers: Vec<(StoragePath, mpsc::UnboundedSender<ChangeEvent>)>,
    failures: HashMap<StoragePath, VfsError>,
    rename_failures: HashMap<StoragePath, VfsError>,
    launched: Vec<StoragePath>,
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
        fake.lock()
            .nodes
            .insert(root, Node { kind: EntryKind::Directory, metadata: EntryMetadata::default(), content: Vec::new() });
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
        let path = self.path(absolute);
        self.insert(&path, node(&path, EntryKind::Directory, Vec::new()));
    }

    /// A file of `size` zero bytes.
    pub fn add_file(&self, absolute: &str, size: u64) {
        self.add_file_with(absolute, &vec![0; usize::try_from(size).unwrap_or_default()]);
    }

    pub fn add_file_with(&self, absolute: &str, content: &[u8]) {
        let path = self.path(absolute);
        self.insert(&path, node(&path, EntryKind::File, content.to_vec()));
    }

    pub fn remove(&self, absolute: &str) {
        let path = self.path(absolute);
        self.remove_tree(&path);
    }

    pub fn emit_rescan(&self) {
        let mut state = self.lock();
        state.watchers.retain(|(_, sender)| sender.unbounded_send(ChangeEvent::Rescan).is_ok());
    }

    /// Every mutating operation on `absolute` fails with `error` from now on.
    pub fn fail_on(&self, absolute: &str, error: VfsError) {
        let path = self.path(absolute);
        self.lock().failures.insert(path, error);
    }

    /// Only renames of `absolute` fail (e.g. `CrossDevice` to force copy + delete).
    pub fn fail_rename(&self, absolute: &str, error: VfsError) {
        let path = self.path(absolute);
        self.lock().rename_failures.insert(path, error);
    }

    #[must_use]
    pub fn exists(&self, absolute: &str) -> bool {
        self.lock().nodes.contains_key(&self.path(absolute))
    }

    #[must_use]
    pub fn contents(&self, absolute: &str) -> Option<Vec<u8>> {
        self.lock().nodes.get(&self.path(absolute)).map(|node| node.content.clone())
    }

    #[must_use]
    pub fn trash_len(&self) -> usize {
        self.lock().trash.len()
    }

    #[must_use]
    pub fn launched(&self) -> Vec<StoragePath> {
        self.lock().launched.clone()
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

    fn insert(&self, path: &StoragePath, node: Node) {
        let mut state = self.lock();
        let event = to_entry(path, &node).map(ChangeEvent::Upserted);
        state.nodes.insert(path.clone(), node);
        if let Some(event) = event {
            notify(&mut state, path, &event);
        }
    }

    fn take_tree(state: &mut State, root: &StoragePath) -> Vec<(StoragePath, Node)> {
        let paths: Vec<StoragePath> = state.nodes.keys().filter(|p| p.is_within(root)).cloned().collect();
        let taken = paths.into_iter().filter_map(|p| state.nodes.remove(&p).map(|n| (p, n))).collect();
        notify(state, root, &ChangeEvent::Removed(root.clone()));
        taken
    }

    fn remove_tree(&self, root: &StoragePath) -> Vec<(StoragePath, Node)> {
        Self::take_tree(&mut self.lock(), root)
    }

    fn put_tree(&self, nodes: Vec<(StoragePath, Node)>, old_root: &StoragePath, new_root: &StoragePath) {
        for (path, node) in nodes {
            if let Some(target) = path.rebase(old_root, new_root) {
                self.insert(&target, relabel(&target, node));
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn check_failure(&self, path: &StoragePath) -> Result<(), VfsError> {
        self.lock().failures.get(path).cloned().map_or(Ok(()), Err)
    }

    fn require_dir(&self, dir: &StoragePath) -> Result<(), VfsError> {
        match self.lock().nodes.get(dir) {
            None => Err(VfsError::NotFound),
            Some(node) if node.kind.is_container() => Ok(()),
            Some(_) => Err(VfsError::NotADirectory),
        }
    }

    fn require_parent_dir(&self, path: &StoragePath) -> Result<(), VfsError> {
        path.parent().map_or(Err(VfsError::NotFound), |parent| self.require_dir(&parent))
    }

    fn require_absent(&self, path: &StoragePath) -> Result<(), VfsError> {
        if self.lock().nodes.contains_key(path) { Err(VfsError::AlreadyExists) } else { Ok(()) }
    }

    fn node(&self, path: &StoragePath) -> Result<Node, VfsError> {
        self.lock().nodes.get(path).cloned().ok_or(VfsError::NotFound)
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

fn node(path: &StoragePath, kind: EntryKind, content: Vec<u8>) -> Node {
    let size = (kind == EntryKind::File).then_some(ByteSize(content.len() as u64));
    relabel(path, Node { kind, metadata: EntryMetadata { size, ..EntryMetadata::default() }, content })
}

/// Recomputes name-derived metadata after a node changes its name.
fn relabel(path: &StoragePath, mut node: Node) -> Node {
    let hidden = path.name().is_some_and(|name| name.as_bytes().first() == Some(&HIDDEN_PREFIX));
    node.metadata.visibility = if hidden { Visibility::Hidden } else { Visibility::Visible };
    node
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
        to_entry(path, &self.node(path)?).ok_or(VfsError::NotFound)
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

impl Copier for FakeVfs {
    async fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<ByteSize, VfsError> {
        self.check_failure(from)?;
        let source = self.node(from)?;
        if source.kind == EntryKind::Directory {
            return Err(VfsError::IsADirectory);
        }
        self.require_absent(to)?;
        self.require_parent_dir(to)?;
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let size = ByteSize(source.content.len() as u64);
        self.insert(to, relabel(to, source));
        progress(size);
        Ok(size)
    }
}

impl Mover for FakeVfs {
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        self.check_failure(from)?;
        if let Some(error) = self.lock().rename_failures.get(from).cloned() {
            return Err(error);
        }
        self.node(from)?;
        self.require_absent(to)?;
        self.require_parent_dir(to)?;
        let tree = self.remove_tree(from);
        self.put_tree(tree, from, to);
        Ok(())
    }
}

impl Remover for FakeVfs {
    async fn remove(&self, path: &StoragePath, _cancel: CancelToken) -> Result<(), VfsError> {
        self.check_failure(path)?;
        self.node(path)?;
        self.remove_tree(path);
        Ok(())
    }
}

impl FolderMaker for FakeVfs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        self.check_failure(path)?;
        self.require_absent(path)?;
        self.require_parent_dir(path)?;
        let folder = node(path, EntryKind::Directory, Vec::new());
        let entry = to_entry(path, &folder).ok_or(VfsError::AlreadyExists)?;
        self.insert(path, folder);
        Ok(entry)
    }
}

impl Trasher for FakeVfs {
    async fn trash(&self, path: &StoragePath) -> Result<TrashTicket, VfsError> {
        self.check_failure(path)?;
        self.node(path)?;
        let name = path.name().cloned().ok_or(VfsError::NotFound)?;
        let mut state = self.lock();
        state.trashed_count += 1;
        let slot = PathSegment::new(&state.trashed_count.to_string())?;
        let trashed = StoragePath::parse(&format!("{FAKE_SCHEME}://{TRASH_ROOT}"))?.join(slot).join(name);
        let tree = Self::take_tree(&mut state, path);
        state.trash.insert(trashed.clone(), tree);
        Ok(TrashTicket { original: path.clone(), trashed })
    }

    async fn restore(&self, ticket: &TrashTicket) -> Result<(), VfsError> {
        if !self.lock().trash.contains_key(&ticket.trashed) {
            return Err(VfsError::NotInTrash);
        }
        self.require_absent(&ticket.original)?;
        self.require_parent_dir(&ticket.original)?;
        let tree = self.lock().trash.remove(&ticket.trashed).unwrap_or_default();
        self.put_tree(tree, &ticket.original, &ticket.original);
        Ok(())
    }
}

impl Launcher for FakeVfs {
    async fn launch(&self, path: &StoragePath) -> Result<(), VfsError> {
        self.node(path)?;
        self.lock().launched.push(path.clone());
        Ok(())
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
        Ok(MountHandle::builder(Capabilities::empty().with(Capability::AtomicRename))
            .lister(fake.clone())
            .watcher(fake.clone())
            .copier(fake.clone())
            .mover(fake.clone())
            .remover(fake.clone())
            .folder_maker(fake.clone())
            .trasher(fake.clone())
            .launcher(fake)
            .build())
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

/// Records every progress snapshot.
#[derive(Debug, Default)]
pub struct RecordingObserver {
    snapshots: Mutex<Vec<JobProgress>>,
}

impl RecordingObserver {
    #[must_use]
    pub fn last(&self) -> Option<JobProgress> {
        self.snapshots.lock().unwrap_or_else(PoisonError::into_inner).last().cloned()
    }
}

impl JobObserver for RecordingObserver {
    fn progress(&self, progress: &JobProgress) {
        self.snapshots.lock().unwrap_or_else(PoisonError::into_inner).push(progress.clone());
    }
}

/// Answers every conflict with a scripted sequence of resolutions (the last repeats).
#[derive(Debug)]
pub struct ScriptedResolver {
    answers: Mutex<Vec<ConflictResolution>>,
    asked: Mutex<Vec<Conflict>>,
}

impl ScriptedResolver {
    #[must_use]
    pub fn new(answers: Vec<ConflictResolution>) -> Self {
        Self { answers: Mutex::new(answers), asked: Mutex::default() }
    }

    #[must_use]
    pub fn times_asked(&self) -> usize {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

impl ConflictResolver for ScriptedResolver {
    fn resolve(&self, conflict: Conflict) -> BoxFuture<'static, ConflictResolution> {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner).push(conflict);
        let mut answers = self.answers.lock().unwrap_or_else(PoisonError::into_inner);
        let answer = if answers.len() > 1 { answers.remove(0) } else { answers[0] };
        future::ready(answer).boxed()
    }
}

#[allow(clippy::panic, reason = "test-support code: invalid fixtures are programmer errors")]
fn unreachable_in_tests(error: &dyn std::error::Error) -> ! {
    panic!("invalid test fixture: {error}")
}
