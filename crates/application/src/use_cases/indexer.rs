//! The background indexer (ADR 0025). One long-running task: it waits until
//! AI search is on and its model installed, then walks the scope, indexing
//! new and changed items and forgetting removed ones, a file at a time with
//! a pause in between, so the machine stays responsive. Commands (settings
//! changed, rescan, shutdown) are noticed between files.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use domain::{
    ContentKind, EntryKind, FileCategory, FileEntry, Fingerprint, IndexDecision, IndexStatus, StoragePath,
    index_decision, passages,
};
use futures::channel::mpsc;
use futures::lock::{Mutex, MutexGuard};
use futures::{StreamExt, TryStreamExt};

use crate::cancel::CancelToken;
use crate::ports::{
    AiSettings, AiSettingsStore, Clock, ContentExtractor, EmbedError, ImageEmbedder, IndexObserver, IndexedItem,
    IndexedPassage, ModelInstaller, ModelPack, MountResolver, SemanticIndex, TextEmbedder,
};

/// Breathing room after each file: indexing is background work.
pub const THROTTLE: Duration = Duration::from_millis(40);
/// How often an up-to-date index looks for changes by itself.
pub const RESCAN_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Snippets are the start of a passage.
const SNIPPET_CHARS: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    /// Settings were saved (enabled, paused, scope, images): re-read them.
    SettingsChanged,
    /// Look for changes now (after file operations, or on request).
    Rescan,
}

/// Steers a running [`Indexer`]; cheap to clone. Dropping every handle
/// stops the indexer.
#[derive(Clone, Debug)]
pub struct IndexerControl {
    commands: mpsc::UnboundedSender<Command>,
    /// Held while settings are loaded, edited and saved, so concurrent
    /// changes don't overwrite each other.
    settings: Arc<Mutex<()>>,
    /// Held by the indexer while it writes an item, and by whoever clears
    /// the index: a clear never lands before a write that was in flight.
    writes: Arc<Mutex<()>>,
}

impl IndexerControl {
    pub(crate) async fn lock_settings(&self) -> MutexGuard<'_, ()> {
        self.settings.lock().await
    }

    /// Waits for an in-flight write; the indexer re-reads its commands before
    /// the next one, so send the command first.
    pub(crate) async fn lock_writes(&self) -> MutexGuard<'_, ()> {
        self.writes.lock().await
    }

    pub fn settings_changed(&self) {
        let _ = self.commands.unbounded_send(Command::SettingsChanged);
    }

    pub fn rescan(&self) {
        let _ = self.commands.unbounded_send(Command::Rescan);
    }
}

/// Everything the indexer works with.
#[derive(Clone)]
pub struct IndexerParts {
    pub mounts: Arc<dyn MountResolver>,
    pub extractor: Arc<dyn ContentExtractor>,
    pub text: Arc<dyn TextEmbedder>,
    pub images: Arc<dyn ImageEmbedder>,
    pub index: Arc<dyn SemanticIndex>,
    pub settings: Arc<dyn AiSettingsStore>,
    pub installer: Arc<dyn ModelInstaller>,
    pub clock: Arc<dyn Clock>,
    pub observer: Arc<dyn IndexObserver>,
}

pub struct Indexer {
    parts: IndexerParts,
    commands: mpsc::UnboundedReceiver<Command>,
    writes: Arc<Mutex<()>>,
    /// A command that arrived during a pass, handled once it stopped.
    pending: Option<Command>,
}

#[must_use]
pub fn indexer(parts: IndexerParts) -> (Indexer, IndexerControl) {
    let (sender, receiver) = mpsc::unbounded();
    let writes = Arc::new(Mutex::new(()));
    let control = IndexerControl { commands: sender, settings: Arc::new(Mutex::new(())), writes: Arc::clone(&writes) };
    (Indexer { parts, commands: receiver, writes, pending: None }, control)
}

/// How a pass ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassOutcome {
    /// Everything in scope is indexed.
    Complete { indexed: u64 },
    /// Settings changed or a rescan was asked for; start again.
    Interrupted,
    /// Off, paused, or the model is missing: nothing to do.
    Idle,
    /// Every control handle is gone.
    Stopped,
}

/// One item waiting to be (re-)indexed.
struct Work {
    entry: FileEntry,
    kind: ContentKind,
}

impl Indexer {
    /// Runs until every [`IndexerControl`] is dropped.
    pub async fn run(mut self) {
        loop {
            match self.pass().await {
                PassOutcome::Stopped => return,
                PassOutcome::Interrupted => {}
                PassOutcome::Idle => {
                    if !self.wait_for_command(None).await {
                        return;
                    }
                }
                PassOutcome::Complete { .. } => {
                    if !self.wait_for_command(Some(RESCAN_INTERVAL)).await {
                        return;
                    }
                }
            }
        }
    }

    /// `false` once every control handle is gone.
    async fn wait_for_command(&mut self, timeout: Option<Duration>) -> bool {
        if self.pending.take().is_some() {
            return true;
        }
        let command = self.commands.next();
        match timeout {
            None => command.await.is_some(),
            Some(timeout) => match futures::future::select(command, self.parts.clock.sleep(timeout)).await {
                futures::future::Either::Left((command, _)) => command.is_some(),
                futures::future::Either::Right(_) => true,
            },
        }
    }

    /// `Err` with how to stop when a command arrived or the channel closed.
    fn check_commands(&mut self) -> Result<(), PassOutcome> {
        match self.commands.try_recv() {
            Ok(command) => {
                self.pending = Some(command);
                Err(PassOutcome::Interrupted)
            }
            Err(mpsc::TryRecvError::Closed) => Err(PassOutcome::Stopped),
            Err(mpsc::TryRecvError::Empty) => Ok(()),
        }
    }

    fn report(&self, status: &IndexStatus) {
        self.parts.observer.status(status);
    }

    async fn indexed_count(&self) -> u64 {
        self.parts.index.count().await.unwrap_or_default()
    }

    /// One pass over the scope, or the reason there is nothing to do.
    pub async fn pass(&mut self) -> PassOutcome {
        self.pending = None;
        let settings = match self.parts.settings.load().await {
            Ok(settings) => settings.unwrap_or_default(),
            Err(error) => {
                self.report(&IndexStatus::Failed(error.to_string()));
                return PassOutcome::Idle;
            }
        };
        if !settings.enabled {
            self.report(&IndexStatus::Off);
            return PassOutcome::Idle;
        }
        if !self.parts.installer.installed(ModelPack::Text) {
            self.report(&IndexStatus::NeedsModel);
            return PassOutcome::Idle;
        }
        if settings.paused {
            self.report(&IndexStatus::Paused { indexed: self.indexed_count().await });
            return PassOutcome::Idle;
        }
        let images = settings.images && self.parts.installer.installed(ModelPack::Images);
        let work = match self.scan(&settings, images).await {
            Ok(work) => work,
            Err(outcome) => return outcome,
        };
        if let Err(outcome) = self.index_all(work).await {
            return outcome;
        }
        let indexed = self.indexed_count().await;
        self.report(&IndexStatus::UpToDate { indexed });
        PassOutcome::Complete { indexed }
    }

    /// Walks every included folder: what needs (re-)indexing is returned;
    /// index entries for items that are gone are removed on the way.
    async fn scan(&mut self, settings: &AiSettings, images: bool) -> Result<Vec<Work>, PassOutcome> {
        let mut work = Vec::new();
        let mut seen_total = 0_u64;
        for root in &settings.scope.include {
            let mut seen = HashSet::new();
            let Ok(lister) = self.parts.mounts.resolve(root.mount()).and_then(|m| m.lister()) else { continue };
            let mut folders = VecDeque::from([root.clone()]);
            while let Some(folder) = folders.pop_front() {
                self.check_commands()?;
                if !settings.scope.covers(&folder) {
                    continue;
                }
                // An unreadable folder is skipped; its items stay indexed until it can be read.
                let Ok(batches) = lister.list_boxed(&folder, CancelToken::never()).try_collect::<Vec<_>>().await else {
                    keep_indexed_below(&folder, &mut seen, &*self.parts.index).await;
                    continue;
                };
                for entry in batches.into_iter().flatten() {
                    if !settings.scope.covers(entry.path()) {
                        continue;
                    }
                    match index_decision(&entry, images) {
                        IndexDecision::Descend => folders.push_back(entry.path().clone()),
                        IndexDecision::Index(kind) => {
                            seen.insert(entry.path().clone());
                            if self.needs_indexing(&entry, kind).await {
                                work.push(Work { entry, kind });
                            }
                        }
                        IndexDecision::Skip => {}
                    }
                }
                seen_total += 1;
                self.report(&IndexStatus::Scanning { seen: seen_total });
            }
            self.forget_missing(root, &seen).await;
        }
        self.forget_out_of_scope(settings).await;
        Ok(work)
    }

    /// Folders taken off the scope (or newly excluded) leave nothing behind.
    async fn forget_out_of_scope(&self, settings: &AiSettings) {
        let Ok(indexed) = self.parts.index.all_locations().await else { return };
        let outside: Vec<StoragePath> =
            indexed.into_iter().filter(|location| !settings.scope.covers(location)).collect();
        if !outside.is_empty() {
            let _ = self.parts.index.remove(outside).await;
        }
    }

    async fn needs_indexing(&self, entry: &FileEntry, kind: ContentKind) -> bool {
        let model = self.model_for(kind);
        match self.parts.index.indexed(entry.path()).await {
            Ok(Some(item)) => item.fingerprint != Fingerprint::of(entry) || item.model != model || item.kind != kind,
            Ok(None) | Err(_) => true,
        }
    }

    fn model_for(&self, kind: ContentKind) -> crate::ports::ModelId {
        match kind {
            ContentKind::Text => self.parts.text.model(),
            ContentKind::Image => self.parts.images.model(),
        }
    }

    async fn forget_missing(&self, root: &StoragePath, seen: &HashSet<StoragePath>) {
        let Ok(indexed) = self.parts.index.locations_within(root).await else { return };
        let gone: Vec<StoragePath> = indexed.into_iter().filter(|location| !seen.contains(location)).collect();
        if !gone.is_empty() {
            let _ = self.parts.index.remove(gone).await;
        }
    }

    async fn index_all(&mut self, work: Vec<Work>) -> Result<(), PassOutcome> {
        let total = work.len() as u64;
        for (done, item) in (0_u64..).zip(work) {
            self.check_commands()?;
            let current = item.entry.name().display_name().into_owned();
            self.report(&IndexStatus::Indexing { done, total, current });
            match self.index_one(&item).await {
                Ok((item, passages)) => {
                    // Settings may have changed (or the index been cleared)
                    // while this item was embedded: then it's dropped.
                    let writes = Arc::clone(&self.writes);
                    let _writing = writes.lock().await;
                    self.check_commands()?;
                    let _ = self.parts.index.upsert(item, passages).await;
                }
                // The model went away (uninstalled mid-pass): stop for now.
                Err(IndexFailure::Model) => return Err(PassOutcome::Interrupted),
                Err(IndexFailure::Retry) => {}
            }
            self.parts.clock.sleep(THROTTLE).await;
        }
        Ok(())
    }

    /// Reads and embeds one item. Items that can't be read now (offline,
    /// locked) are retried next pass; items with nothing to index are
    /// remembered as such, so they aren't read again until they change. The
    /// caller writes the result.
    async fn index_one(&self, work: &Work) -> Result<(IndexedItem, Vec<IndexedPassage>), IndexFailure> {
        let entry = &work.entry;
        let materializer = self.parts.mounts.resolve(entry.path().mount()).and_then(|m| m.materializer());
        let file = match materializer {
            Ok(materializer) => materializer.local_file_boxed(entry.path()).await,
            Err(error) => Err(error),
        }
        .map_err(|_| IndexFailure::Retry)?;
        let category = FileCategory::of(entry);
        let passages = match work.kind {
            ContentKind::Text => {
                let text = self.parts.extractor.extract(&file, category).await.map_err(|_| IndexFailure::Retry)?;
                let chunks = text.as_deref().map(passages).unwrap_or_default();
                let vectors = if chunks.is_empty() {
                    Vec::new()
                } else {
                    self.parts.text.embed_passages(chunks.clone()).await.map_err(IndexFailure::from)?
                };
                chunks
                    .into_iter()
                    .zip(vectors)
                    .map(|(text, vector)| IndexedPassage { snippet: snippet(&text), vector })
                    .collect()
            }
            ContentKind::Image => match self.parts.images.embed_image(file).await.map_err(IndexFailure::from)? {
                Some(vector) => vec![IndexedPassage { snippet: String::new(), vector }],
                None => Vec::new(),
            },
        };
        let item = IndexedItem {
            location: entry.path().clone(),
            kind: work.kind,
            category,
            fingerprint: Fingerprint::of(entry),
            model: self.model_for(work.kind),
        };
        Ok((item, passages))
    }
}

enum IndexFailure {
    /// Try again next pass.
    Retry,
    /// The model isn't usable; stop the pass.
    Model,
}

impl From<EmbedError> for IndexFailure {
    fn from(error: EmbedError) -> Self {
        match error {
            EmbedError::Missing => Self::Model,
            EmbedError::Failed(_) => Self::Retry,
        }
    }
}

/// Everything already indexed below an unreadable folder counts as seen.
async fn keep_indexed_below(folder: &StoragePath, seen: &mut HashSet<StoragePath>, index: &dyn SemanticIndex) {
    if let Ok(locations) = index.locations_within(folder).await {
        seen.extend(locations);
    }
}

fn snippet(text: &str) -> String {
    let mut snippet: String = text.chars().take(SNIPPET_CHARS).collect();
    if text.chars().count() > SNIPPET_CHARS {
        snippet.push('…');
    }
    snippet
}

/// An index entry as a file entry, for showing results like any listing.
#[must_use]
pub fn result_entry(item: &IndexedItem) -> Option<FileEntry> {
    let (parent, name) = item.location.parent().zip(item.location.name().cloned())?;
    let metadata = domain::EntryMetadata {
        size: item.fingerprint.size,
        modified: item.fingerprint.modified,
        ..domain::EntryMetadata::default()
    };
    Some(FileEntry::new(&parent, name, EntryKind::File, metadata))
}

impl std::fmt::Debug for Indexer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Indexer").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for IndexerParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexerParts").finish_non_exhaustive()
    }
}
