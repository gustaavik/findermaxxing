//! In-memory fakes of the ports. They pass the same contract suite as real
//! adapters (see `vfs-contract-tests`), which keeps use-case tests honest.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use domain::{
    AccountId, BucketName, ByteSize, Capabilities, Capability, ChangeEvent, Conflict, ConflictResolution, CostDecision,
    CostWarning, EntryBatch, EntryKind, EntryMetadata, FileEntry, JobProgress, LIST_BATCH_SIZE, MountId, PathSegment,
    S3Account, SavedSearch, SavedSearchId, Scheme, StoragePath, TrashTicket, VfsError, Visibility,
};
use futures::channel::mpsc;
use futures::future::{self, BoxFuture};
use futures::stream::{self, Stream};
use futures::{FutureExt, StreamExt};

use crate::cancel::CancelToken;
use crate::ports::{
    AccountStore, ArchiveInspector, ArchiveItem, ArchiveListing, BucketInfo, ByteStream, Clock, ConflictResolver,
    Copier, CostConfirmer, CredentialError, CredentialKey, CredentialStore, FavoritesStore, FolderMaker, JobObserver,
    Launcher, Lister, LocalFile, LocalFileOpener, Materializer, MountHandle, Mover, NewS3Account, NewSavedSearch,
    PreviewFailure, PreviewTheme, ProgressCallback, ProviderFactory, Reader, Remover, RenderedText, S3Connector,
    SavedSearchStore, Secret, StoreError, TextRenderer, Trasher, Volume, VolumeLister, Watcher, WriteSink, Writer,
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

/// How a fake mount presents itself to the application.
#[derive(Debug, Clone)]
struct FakeTraits {
    scheme: String,
    /// Behave like a billed remote provider: `Metered`, a region, no Trash,
    /// no launcher (items open via their materialised copy).
    remote: bool,
    region: Option<String>,
}

/// An in-memory filesystem under the `fake://` scheme. Clones share state.
#[derive(Debug, Clone)]
pub struct FakeVfs {
    state: Arc<Mutex<State>>,
    traits: Arc<FakeTraits>,
}

impl Default for FakeVfs {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeVfs {
    #[must_use]
    pub fn new() -> Self {
        Self::with_traits(FakeTraits { scheme: FAKE_SCHEME.to_owned(), remote: false, region: None })
    }

    /// A second, remote-like provider (think S3) under its own scheme.
    #[must_use]
    pub fn remote(scheme: &str, region: Option<&str>) -> Self {
        Self::with_traits(FakeTraits { scheme: scheme.to_owned(), remote: true, region: region.map(str::to_owned) })
    }

    fn with_traits(traits: FakeTraits) -> Self {
        let fake = Self { state: Arc::default(), traits: Arc::new(traits) };
        let root = fake.path("/");
        fake.lock()
            .nodes
            .insert(root, Node { kind: EntryKind::Directory, metadata: EntryMetadata::default(), content: Vec::new() });
        fake
    }

    /// `fake://` + `absolute`. Invalid input is a bug in the test itself.
    #[must_use]
    pub fn path(&self, absolute: &str) -> StoragePath {
        match StoragePath::parse(&format!("{}://{absolute}", self.traits.scheme)) {
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
        let trashed = self.path(TRASH_ROOT).join(slot).join(name);
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

/// Fake items have no real local file; previews get a path that names the item.
pub const FAKE_LOCAL_ROOT: &str = "/fake-local";

impl Materializer for FakeVfs {
    async fn local_file(&self, item: &StoragePath) -> Result<LocalFile, VfsError> {
        self.node(item)?;
        let root = format!("{FAKE_LOCAL_ROOT}/{}", self.traits.scheme);
        Ok(LocalFile::new(std::path::PathBuf::from(format!("{root}{}", item.path_string()))))
    }
}

#[derive(Debug)]
struct FakeFactory {
    fake: FakeVfs,
}

impl ProviderFactory for FakeFactory {
    fn scheme(&self) -> Scheme {
        match Scheme::new(&self.fake.traits.scheme) {
            Ok(scheme) => scheme,
            Err(error) => unreachable_in_tests(&error),
        }
    }

    fn open(&self, _mount: &MountId) -> Result<MountHandle, VfsError> {
        let fake = Arc::new(self.fake.clone());
        let traits = &self.fake.traits;
        let semantics = if traits.remote {
            Capabilities::empty().with(Capability::Metered)
        } else {
            Capabilities::empty().with(Capability::AtomicRename)
        };
        let builder = MountHandle::builder(semantics)
            .region(traits.region.clone())
            .lister(fake.clone())
            .watcher(fake.clone())
            .copier(fake.clone())
            .mover(fake.clone())
            .remover(fake.clone())
            .folder_maker(fake.clone())
            .reader(fake.clone())
            .writer(fake.clone())
            .materializer(fake.clone());
        Ok(if traits.remote { builder } else { builder.trasher(fake.clone()).launcher(fake) }.build())
    }
}

/// Fake reads arrive in chunks this small so multi-chunk paths get exercised.
const FAKE_READ_CHUNK: usize = 4;

impl Reader for FakeVfs {
    async fn read(&self, path: &StoragePath, _cancel: CancelToken) -> Result<ByteStream, VfsError> {
        self.check_failure(path)?;
        let node = self.node(path)?;
        if node.kind.is_container() {
            return Err(VfsError::IsADirectory);
        }
        let chunks: Vec<Result<Vec<u8>, VfsError>> =
            node.content.chunks(FAKE_READ_CHUNK).map(|chunk| Ok(chunk.to_vec())).collect();
        Ok(stream::iter(chunks).boxed())
    }
}

impl Writer for FakeVfs {
    async fn create(&self, path: &StoragePath, _size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        self.check_failure(path)?;
        self.require_absent(path)?;
        self.require_parent_dir(path)?;
        Ok(Box::new(FakeSink { fake: self.clone(), path: path.clone(), buffer: Vec::new() }))
    }
}

/// Buffers until commit, so nothing is visible before then.
struct FakeSink {
    fake: FakeVfs,
    path: StoragePath,
    buffer: Vec<u8>,
}

impl WriteSink for FakeSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        self.buffer.extend(chunk);
        future::ready(Ok(())).boxed()
    }

    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        let result = self.fake.require_absent(&self.path).map(|()| {
            let committed = node(&self.path, EntryKind::File, self.buffer);
            self.fake.insert(&self.path, committed);
        });
        future::ready(result).boxed()
    }

    fn abort(self: Box<Self>) -> BoxFuture<'static, ()> {
        future::ready(()).boxed()
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

/// Renders every file as `<pre>name</pre>`; files ending in `.bin` count as binary.
#[derive(Debug, Default)]
pub struct StubTextRenderer;

impl TextRenderer for StubTextRenderer {
    fn render(
        &self,
        file: &LocalFile,
        _theme: PreviewTheme,
    ) -> BoxFuture<'static, Result<Option<RenderedText>, PreviewFailure>> {
        let path = file.path().display().to_string();
        let binary = path.ends_with(".bin");
        let text =
            RenderedText { html: format!("<pre>{path}</pre>"), language: "Plain Text".to_owned(), truncated: false };
        future::ready(Ok((!binary).then_some(text))).boxed()
    }
}

/// Lists one entry named after the archive.
#[derive(Debug, Default)]
pub struct StubArchiveInspector;

impl ArchiveInspector for StubArchiveInspector {
    fn list(&self, file: &LocalFile, _limit: usize) -> BoxFuture<'static, Result<ArchiveListing, PreviewFailure>> {
        let item = ArchiveItem { path: file.path().display().to_string(), size: None, is_folder: false };
        future::ready(Ok(ArchiveListing { items: vec![item], truncated: false })).boxed()
    }
}

/// Saved searches in memory, ids counting up from 1.
#[derive(Debug, Default)]
pub struct InMemorySavedSearches {
    searches: Mutex<Vec<SavedSearch>>,
}

impl SavedSearchStore for InMemorySavedSearches {
    fn list(&self) -> BoxFuture<'static, Result<Vec<SavedSearch>, StoreError>> {
        let mut all = self.searches.lock().unwrap_or_else(PoisonError::into_inner).clone();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        future::ready(Ok(all)).boxed()
    }

    fn save(&self, search: NewSavedSearch) -> BoxFuture<'static, Result<SavedSearch, StoreError>> {
        let mut searches = self.searches.lock().unwrap_or_else(PoisonError::into_inner);
        let id = SavedSearchId(searches.iter().map(|s| s.id.0).max().unwrap_or_default() + 1);
        let saved = SavedSearch { id, name: search.name, scope: search.scope, query: search.query };
        searches.push(saved.clone());
        future::ready(Ok(saved)).boxed()
    }

    fn delete(&self, id: SavedSearchId) -> BoxFuture<'static, Result<(), StoreError>> {
        self.searches.lock().unwrap_or_else(PoisonError::into_inner).retain(|s| s.id != id);
        future::ready(Ok(())).boxed()
    }
}

/// Answers every cost question the same way and remembers what it was shown.
#[derive(Debug)]
pub struct ScriptedCosts {
    decision: CostDecision,
    shown: Mutex<Vec<CostWarning>>,
}

impl ScriptedCosts {
    #[must_use]
    pub fn new(decision: CostDecision) -> Self {
        Self { decision, shown: Mutex::default() }
    }

    #[must_use]
    pub fn warnings(&self) -> Vec<CostWarning> {
        self.shown.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl CostConfirmer for ScriptedCosts {
    fn confirm(&self, warning: CostWarning) -> BoxFuture<'static, CostDecision> {
        self.shown.lock().unwrap_or_else(PoisonError::into_inner).push(warning);
        future::ready(self.decision).boxed()
    }
}

/// Accounts in memory.
#[derive(Debug, Default)]
pub struct InMemoryAccounts {
    accounts: Mutex<Vec<S3Account>>,
}

impl InMemoryAccounts {
    #[must_use]
    pub fn all(&self) -> Vec<S3Account> {
        self.accounts.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl AccountStore for InMemoryAccounts {
    fn list_s3(&self) -> BoxFuture<'static, Result<Vec<S3Account>, StoreError>> {
        future::ready(Ok(self.all())).boxed()
    }

    fn add_s3(&self, account: NewS3Account) -> BoxFuture<'static, Result<S3Account, StoreError>> {
        let mut accounts = self.accounts.lock().unwrap_or_else(PoisonError::into_inner);
        let id = AccountId(accounts.iter().map(|a| a.id.0).max().unwrap_or_default() + 1);
        let added = S3Account {
            id,
            name: account.name,
            endpoint: account.endpoint,
            region: account.region,
            auth: account.auth,
            addressing: account.addressing,
            skip_cost_warnings: false,
            extra_buckets: Vec::new(),
        };
        accounts.push(added.clone());
        future::ready(Ok(added)).boxed()
    }

    fn remove(&self, id: AccountId) -> BoxFuture<'static, Result<(), StoreError>> {
        self.accounts.lock().unwrap_or_else(PoisonError::into_inner).retain(|a| a.id != id);
        future::ready(Ok(())).boxed()
    }

    fn set_skip_cost_warnings(&self, id: AccountId, skip: bool) -> BoxFuture<'static, Result<(), StoreError>> {
        for account in self.accounts.lock().unwrap_or_else(PoisonError::into_inner).iter_mut().filter(|a| a.id == id) {
            account.skip_cost_warnings = skip;
        }
        future::ready(Ok(())).boxed()
    }

    fn add_bucket(&self, id: AccountId, bucket: BucketName) -> BoxFuture<'static, Result<(), StoreError>> {
        for account in self.accounts.lock().unwrap_or_else(PoisonError::into_inner).iter_mut().filter(|a| a.id == id) {
            if !account.extra_buckets.contains(&bucket) {
                account.extra_buckets.push(bucket.clone());
            }
        }
        future::ready(Ok(())).boxed()
    }
}

/// A keychain in memory.
#[derive(Debug, Default)]
pub struct InMemoryKeychain {
    secrets: Mutex<HashMap<CredentialKey, Secret>>,
}

impl InMemoryKeychain {
    #[must_use]
    pub fn len(&self) -> usize {
        self.secrets.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl CredentialStore for InMemoryKeychain {
    fn store(&self, key: &CredentialKey, secret: &Secret) -> BoxFuture<'static, Result<(), CredentialError>> {
        self.secrets.lock().unwrap_or_else(PoisonError::into_inner).insert(key.clone(), secret.clone());
        future::ready(Ok(())).boxed()
    }

    fn load(&self, key: &CredentialKey) -> BoxFuture<'static, Result<Option<Secret>, CredentialError>> {
        future::ready(Ok(self.secrets.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned())).boxed()
    }

    fn delete(&self, key: &CredentialKey) -> BoxFuture<'static, Result<(), CredentialError>> {
        self.secrets.lock().unwrap_or_else(PoisonError::into_inner).remove(key);
        future::ready(Ok(())).boxed()
    }
}

/// Connects any account whose secret (if any) is `"correct"`; every
/// connected account has the given buckets (or, if `deny_listing`, may not list them).
#[derive(Debug, Default)]
pub struct FakeS3Connector {
    buckets: Vec<String>,
    connected: Mutex<Vec<AccountId>>,
    deny_listing: bool,
}

impl FakeS3Connector {
    pub const VALID_SECRET: &'static str = "correct";

    #[must_use]
    pub fn with_buckets(buckets: &[&str]) -> Self {
        Self {
            buckets: buckets.iter().map(|b| (*b).to_owned()).collect(),
            connected: Mutex::default(),
            deny_listing: false,
        }
    }

    /// Like a scoped IAM key: connects, but ListBuckets is denied.
    #[must_use]
    pub fn denying_listing() -> Self {
        Self { deny_listing: true, ..Self::default() }
    }

    #[must_use]
    pub fn connected(&self) -> Vec<AccountId> {
        self.connected.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl S3Connector for FakeS3Connector {
    fn connect(&self, account: &S3Account, secret: Option<Secret>) -> BoxFuture<'static, Result<(), VfsError>> {
        let valid = secret.as_ref().is_none_or(|s| s.expose() == Self::VALID_SECRET);
        let result = if valid {
            self.connected.lock().unwrap_or_else(PoisonError::into_inner).push(account.id);
            Ok(())
        } else {
            Err(VfsError::PermissionDenied)
        };
        future::ready(result).boxed()
    }

    fn disconnect(&self, id: AccountId) {
        self.connected.lock().unwrap_or_else(PoisonError::into_inner).retain(|c| *c != id);
    }

    fn list_buckets(&self, id: AccountId) -> BoxFuture<'static, Result<Vec<BucketInfo>, VfsError>> {
        let connected = self.connected.lock().unwrap_or_else(PoisonError::into_inner).contains(&id);
        let buckets = self
            .buckets
            .iter()
            .filter_map(|name| BucketName::new(name).ok())
            .map(|name| BucketInfo { name, region: None })
            .collect();
        let result = match (connected, self.deny_listing) {
            (false, _) => Err(VfsError::NotFound),
            (true, true) => Err(VfsError::PermissionDenied),
            (true, false) => Ok(buckets),
        };
        future::ready(result).boxed()
    }
}

/// Favourites in memory, in insertion order.
#[derive(Debug, Default)]
pub struct InMemoryFavorites {
    locations: Mutex<Vec<StoragePath>>,
}

impl FavoritesStore for InMemoryFavorites {
    fn list(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>> {
        future::ready(Ok(self.locations.lock().unwrap_or_else(PoisonError::into_inner).clone())).boxed()
    }

    fn add(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>> {
        let mut locations = self.locations.lock().unwrap_or_else(PoisonError::into_inner);
        if !locations.contains(&location) {
            locations.push(location);
        }
        future::ready(Ok(())).boxed()
    }

    fn remove(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>> {
        self.locations.lock().unwrap_or_else(PoisonError::into_inner).retain(|l| *l != location);
        future::ready(Ok(())).boxed()
    }
}

/// A fixed list of volumes.
#[derive(Debug, Default)]
pub struct FixedVolumes(pub Vec<Volume>);

impl VolumeLister for FixedVolumes {
    fn volumes(&self) -> BoxFuture<'static, Vec<Volume>> {
        future::ready(self.0.clone()).boxed()
    }
}

/// Records opened local files.
#[derive(Debug, Default)]
pub struct RecordingOpener {
    opened: Mutex<Vec<LocalFile>>,
}

impl RecordingOpener {
    #[must_use]
    pub fn opened(&self) -> Vec<LocalFile> {
        self.opened.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl LocalFileOpener for RecordingOpener {
    fn open(&self, file: &LocalFile) -> BoxFuture<'static, Result<(), VfsError>> {
        self.opened.lock().unwrap_or_else(PoisonError::into_inner).push(file.clone());
        future::ready(Ok(())).boxed()
    }
}

#[allow(clippy::panic, reason = "test-support code: invalid fixtures are programmer errors")]
fn unreachable_in_tests(error: &dyn std::error::Error) -> ! {
    panic!("invalid test fixture: {error}")
}
