//! Copy and move of top-level items into one destination folder, on the same
//! provider or across providers, with conflict resolution, cost confirmation,
//! progress, cancellation and an exact undo record.
//!
//! Strategy per file: a same-provider copy uses the provider's `Copier`
//! (APFS clone, S3 server-side copy) and falls back to streaming when it
//! can't; cross-provider copies always stream `Reader` → `Writer`. Large
//! streamed writes are journaled so an interrupted job can resume (ADR 0019).

use std::sync::Arc;

use domain::{
    ByteSize, Capability, Conflict, ConflictChoice, ConflictMemory, CostDecision, CostPolicy, EntryKind,
    FIRST_DUPLICATE_NUMBER, FileEntry, Operation, Relocation, StoragePath, TransferEstimate, TrashTicket, VfsError,
    numbered_name,
};
use futures::StreamExt;

use super::context::JobContext;
use super::error::{OperationError, OperationReport};
use super::progress::ProgressTracker;
use super::resume::{Existing, Previous, RESUMABLE_SIZE, Resumption, finished_earlier};
use super::scan::{ScannedTree, scan};
use crate::cancel::CancelToken;
use crate::ports::{InFlightWrite, MountHandle, MountResolver, WriteSink};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferMode {
    Copy,
    Move,
}

/// Runs a whole transfer. The report always contains what was completed.
pub(crate) async fn run(
    mounts: &dyn MountResolver,
    mode: TransferMode,
    sources: &[StoragePath],
    destination: &StoragePath,
    policy: CostPolicy,
    ctx: &JobContext,
    resumption: &Resumption,
) -> OperationReport {
    let mut transfer = match Transfer::new(mounts, mode, destination, policy, ctx, resumption) {
        Ok(transfer) => transfer,
        Err(error) => return OperationReport::new(Operation::sequence(Vec::new()), Err(error)),
    };
    let result = transfer.execute(sources).await;
    OperationReport::new(transfer.finish(), result)
}

struct Prepared {
    entry: FileEntry,
    source: MountHandle,
    same_mount: bool,
    /// Present whenever bytes will be copied (every copy, and moves across
    /// providers): progress totals need every descendant up front.
    tree: Option<ScannedTree>,
}

impl Prepared {
    fn items(&self) -> u64 {
        self.tree.as_ref().map_or(1, ScannedTree::item_count)
    }

    fn bytes(&self) -> ByteSize {
        self.tree.as_ref().map_or(ByteSize(0), |tree| tree.bytes)
    }
}

enum Placement {
    At(StoragePath),
    Skip,
    Stop,
}

struct Transfer<'a> {
    mode: TransferMode,
    mounts: &'a dyn MountResolver,
    dest: MountHandle,
    destination: &'a StoragePath,
    policy: CostPolicy,
    ctx: &'a JobContext,
    resumption: &'a Resumption,
    tracker: Arc<ProgressTracker>,
    memory: ConflictMemory,
    replaced: Vec<TrashTicket>,
    deleted: Vec<StoragePath>,
    relocations: Vec<Relocation>,
}

impl<'a> Transfer<'a> {
    fn new(
        mounts: &'a dyn MountResolver,
        mode: TransferMode,
        destination: &'a StoragePath,
        policy: CostPolicy,
        ctx: &'a JobContext,
        resumption: &'a Resumption,
    ) -> Result<Self, OperationError> {
        let dest = mounts.resolve(destination.mount()).map_err(OperationError::on(destination))?;
        Ok(Self {
            mode,
            mounts,
            dest,
            destination,
            policy,
            ctx,
            resumption,
            tracker: ProgressTracker::new(Arc::clone(&ctx.observer)),
            memory: ConflictMemory::default(),
            replaced: Vec::new(),
            deleted: Vec::new(),
            relocations: Vec::new(),
        })
    }

    fn finish(self) -> Operation {
        let transferred = match self.mode {
            TransferMode::Copy => Operation::Copied(self.relocations),
            TransferMode::Move => Operation::Moved(self.relocations),
        };
        Operation::sequence(vec![Operation::Trashed(self.replaced), Operation::Deleted(self.deleted), transferred])
    }

    async fn execute(&mut self, sources: &[StoragePath]) -> Result<(), OperationError> {
        let prepared = self.prepare(sources).await?;
        self.confirm_costs(&prepared).await?;
        for item in prepared {
            if self.ctx.cancel.is_cancelled() {
                return Err(OperationError::Cancelled);
            }
            match self.placement(&item.entry).await? {
                Placement::At(target) => self.transfer_one(item, target).await?,
                Placement::Skip => self.tracker.skipped(item.items(), item.bytes()),
                Placement::Stop => return Err(OperationError::Stopped),
            }
        }
        Ok(())
    }

    async fn prepare(&self, sources: &[StoragePath]) -> Result<Vec<Prepared>, OperationError> {
        let mut prepared = Vec::with_capacity(sources.len());
        for source in sources {
            let on_source = OperationError::on(source);
            let handle = self.mounts.resolve(source.mount()).map_err(OperationError::on(source))?;
            let lister = handle.lister().map_err(OperationError::on(source))?;
            let entry = match lister.stat_boxed(source).await {
                // Continuing a move: the earlier run already moved this one.
                Err(VfsError::NotFound) if self.resumption.existing == Existing::EarlierRun => continue,
                other => other.map_err(on_source)?,
            };
            if entry.kind() == EntryKind::Directory && self.destination.is_within(source) {
                return Err(OperationError::IntoItself { item: source.clone() });
            }
            let same_mount = source.mount() == self.destination.mount();
            let tree = if self.mode == TransferMode::Copy || !same_mount {
                Some(scan(&*lister, entry.clone(), &self.ctx.cancel).await.map_err(OperationError::on(source))?)
            } else {
                None
            };
            prepared.push(Prepared { entry, source: handle, same_mount, tree });
        }
        let items = prepared.iter().map(Prepared::items).sum();
        let bytes = ByteSize(prepared.iter().map(|p| p.bytes().0).sum());
        self.tracker.begin(items, bytes);
        Ok(prepared)
    }

    /// Asks before transfers that may be billed (large, or across regions).
    async fn confirm_costs(&self, prepared: &[Prepared]) -> Result<(), OperationError> {
        let metered_source = prepared.iter().find(|p| p.source.capabilities().contains(Capability::Metered));
        let dest_metered = self.dest.capabilities().contains(Capability::Metered);
        let estimate = TransferEstimate {
            bytes: ByteSize(prepared.iter().map(|p| p.bytes().0).sum()),
            items: prepared.iter().map(Prepared::items).sum(),
            metered: dest_metered || metered_source.is_some(),
            source_region: metered_source.and_then(|p| p.source.region().map(str::to_owned)),
            destination_region: if dest_metered { self.dest.region().map(str::to_owned) } else { None },
        };
        let Some(warning) = self.policy.assess(&estimate) else { return Ok(()) };
        match self.ctx.costs.confirm(warning).await {
            CostDecision::Cancel => Err(OperationError::Stopped),
            CostDecision::Proceed | CostDecision::ProceedAndStopAsking => Ok(()),
        }
    }

    async fn placement(&mut self, entry: &FileEntry) -> Result<Placement, OperationError> {
        let candidate = self.destination.join(entry.name().clone());
        if &candidate == entry.path() {
            // Copying into its own folder duplicates (Finder behaviour); moving there is a no-op.
            return match self.mode {
                TransferMode::Copy => self.free_name(entry).await.map(Placement::At),
                TransferMode::Move => Ok(Placement::Skip),
            };
        }
        match self.existing(&candidate).await? {
            None => Ok(Placement::At(candidate)),
            Some(existing)
                if self.resumption.existing == Existing::EarlierRun && finished_earlier(entry, &existing) =>
            {
                // Folders are merged into; a complete file is done.
                Ok(if entry.kind() == EntryKind::Directory { Placement::At(candidate) } else { Placement::Skip })
            }
            Some(existing) => self.resolve_conflict(entry, existing).await,
        }
    }

    async fn existing(&self, path: &StoragePath) -> Result<Option<FileEntry>, OperationError> {
        let lister = self.dest.lister().map_err(OperationError::on(path))?;
        match lister.stat_boxed(path).await {
            Ok(entry) => Ok(Some(entry)),
            Err(VfsError::NotFound) => Ok(None),
            Err(error) => Err(OperationError::on(path)(error)),
        }
    }

    async fn resolve_conflict(&mut self, entry: &FileEntry, existing: FileEntry) -> Result<Placement, OperationError> {
        let choice = match self.memory.remembered() {
            Some(choice) => choice,
            None => {
                let replace_is_permanent = !self.dest.capabilities().contains(Capability::Trash);
                let conflict = Conflict { incoming: entry.clone(), existing: existing.clone(), replace_is_permanent };
                let resolution = self.ctx.resolver.resolve(conflict).await;
                self.memory.record(resolution);
                resolution.choice
            }
        };
        match choice {
            ConflictChoice::Replace => self.replace(entry, &existing).await.map(Placement::At),
            ConflictChoice::Skip => Ok(Placement::Skip),
            ConflictChoice::KeepBoth => self.free_name(entry).await.map(Placement::At),
            ConflictChoice::Stop => Ok(Placement::Stop),
        }
    }

    /// The replaced item goes to the Trash, so undo can bring it back. Where
    /// there is no Trash (S3) it is deleted; the conflict prompt said so.
    async fn replace(&mut self, entry: &FileEntry, existing: &FileEntry) -> Result<StoragePath, OperationError> {
        let target = existing.path();
        if entry.path().is_within(target) {
            return Err(OperationError::IntoItself { item: target.clone() });
        }
        if let Ok(trasher) = self.dest.trasher() {
            let ticket = trasher.trash_boxed(target).await.map_err(OperationError::on(target))?;
            self.replaced.push(ticket);
        } else {
            let remover = self.dest.remover().map_err(OperationError::on(target))?;
            remover.remove_boxed(target, self.ctx.cancel.clone()).await.map_err(OperationError::on(target))?;
            self.deleted.push(target.clone());
        }
        Ok(target.clone())
    }

    async fn free_name(&self, entry: &FileEntry) -> Result<StoragePath, OperationError> {
        let mut number = FIRST_DUPLICATE_NUMBER;
        loop {
            let name = numbered_name(entry.name(), entry.kind(), number)
                .map_err(|error| OperationError::on(entry.path())(error.into()))?;
            let candidate = self.destination.join(name);
            if self.existing(&candidate).await?.is_none() {
                return Ok(candidate);
            }
            number = number.saturating_add(1);
        }
    }

    async fn transfer_one(&mut self, item: Prepared, target: StoragePath) -> Result<(), OperationError> {
        let relocation = Relocation { from: item.entry.path().clone(), to: target.clone() };
        let route =
            Route { source: &item.source, dest: &self.dest, same_mount: item.same_mount, resume: self.resumption };
        let cancel = &self.ctx.cancel;
        let result = match (self.mode, &item.tree) {
            (TransferMode::Copy, Some(tree)) => {
                copy_tree(&route, tree, &target, &self.tracker, cancel).await.map_err(|failure| {
                    // A partial copy is still recorded so that undo cleans it up.
                    if failure.root_created {
                        self.relocations.push(relocation.clone());
                    }
                    failure.error
                })
            }
            (TransferMode::Move, Some(tree)) => {
                move_across(&route, &item.entry, tree, &target, &self.tracker, cancel).await
            }
            (_, None) => move_item(&self.dest, &item.entry, &target, &self.tracker, cancel).await,
        };
        result.map(|()| self.relocations.push(relocation))
    }
}

static NO_RESUMPTION: Resumption = Resumption::none();

/// Where bytes come from and go to.
pub(crate) struct Route<'a> {
    pub source: &'a MountHandle,
    pub dest: &'a MountHandle,
    pub same_mount: bool,
    pub resume: &'a Resumption,
}

pub(crate) struct TreeFailure {
    pub root_created: bool,
    pub error: OperationError,
}

/// Recreates `tree` at `target`: folders via the destination's
/// `FolderMaker`, files by provider copy or streaming.
pub(crate) async fn copy_tree(
    route: &Route<'_>,
    tree: &ScannedTree,
    target: &StoragePath,
    tracker: &Arc<ProgressTracker>,
    cancel: &CancelToken,
) -> Result<(), TreeFailure> {
    let mut root_created = false;
    for item in &tree.items {
        let fail = |error: VfsError| TreeFailure { root_created, error: OperationError::on(item.path())(error) };
        let destination = item.path().rebase(tree.root.path(), target).ok_or_else(|| fail(VfsError::NotFound))?;
        tracker.item_started(item.name().into());
        let result = if route.resume.existing == Existing::EarlierRun && done_earlier(route, item, &destination).await {
            tracker.byte_callback()(item.metadata().size.unwrap_or_default());
            Ok(())
        } else if item.kind() == EntryKind::Directory {
            match route.dest.folder_maker() {
                Ok(maker) => maker.create_folder_boxed(&destination).await.map(drop),
                Err(error) => Err(error),
            }
        } else {
            copy_file(route, item, &destination, tracker, cancel).await
        };
        result.map_err(fail)?;
        root_created = true;
        tracker.item_finished();
    }
    Ok(())
}

async fn done_earlier(route: &Route<'_>, item: &FileEntry, destination: &StoragePath) -> bool {
    let Ok(lister) = route.dest.lister() else { return false };
    lister.stat_boxed(destination).await.is_ok_and(|existing| finished_earlier(item, &existing))
}

async fn copy_file(
    route: &Route<'_>,
    item: &FileEntry,
    destination: &StoragePath,
    tracker: &Arc<ProgressTracker>,
    cancel: &CancelToken,
) -> Result<(), VfsError> {
    if route.same_mount
        && let Ok(copier) = route.dest.copier()
    {
        match copier.copy_item_boxed(item.path(), destination, tracker.byte_callback(), cancel.clone()).await {
            // E.g. S3 server-side copy above 5 GB: fall through to streaming.
            Err(VfsError::Unsupported(_)) => {}
            other => return other.map(drop),
        }
    }
    stream_file(route, item, destination, tracker, cancel).await
}

/// Reader → Writer, chunk by chunk. Aborts the write on any error so the
/// destination never holds a partial file, unless the write was journaled
/// for resuming and the job is merely interrupted (dropped).
async fn stream_file(
    route: &Route<'_>,
    item: &FileEntry,
    destination: &StoragePath,
    tracker: &Arc<ProgressTracker>,
    cancel: &CancelToken,
) -> Result<(), VfsError> {
    let reader = route.source.reader()?;
    let report = tracker.byte_callback();
    let (mut sink, offset) = open_sink(route, item, destination).await?;
    let opened = if offset.0 > 0 {
        reader.read_from_boxed(item.path(), offset, cancel.clone()).await
    } else {
        reader.read_boxed(item.path(), cancel.clone()).await
    };
    let mut chunks = match opened {
        Ok(chunks) => chunks,
        Err(error) => {
            sink.abort().await;
            return Err(error);
        }
    };
    report(offset);
    let mut journal = Checkpoint::new(route, item, destination, offset.0 > 0);
    let result = match pump(route, &mut chunks, &mut *sink, &mut journal, &report, cancel).await {
        Ok(()) => sink.commit().await,
        Err(error) => {
            sink.abort().await;
            Err(error)
        }
    };
    journal.finish(route).await;
    result
}

/// Moves every chunk from `chunks` into `sink`, journaling it once it can
/// be resumed. The caller commits or aborts.
async fn pump(
    route: &Route<'_>,
    chunks: &mut crate::ports::ByteStream,
    sink: &mut dyn WriteSink,
    journal: &mut Checkpoint,
    report: &crate::ports::ProgressCallback,
    cancel: &CancelToken,
) -> Result<(), VfsError> {
    while let Some(chunk) = chunks.next().await {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let chunk = chunk?;
        let length = ByteSize(chunk.len() as u64);
        sink.write(chunk).await?;
        report(length);
        journal.record(route, sink).await;
    }
    if cancel.is_cancelled() { Err(VfsError::Cancelled) } else { Ok(()) }
}

/// A new sink, or the interrupted run's partial write continued.
async fn open_sink(
    route: &Route<'_>,
    item: &FileEntry,
    destination: &StoragePath,
) -> Result<(Box<dyn WriteSink>, ByteSize), VfsError> {
    let writer = route.dest.writer()?;
    match route.resume.take_previous(item, destination) {
        Some(Previous::Continue(state)) => match writer.resume_boxed(destination, &state).await {
            Ok(resumed) => return Ok((resumed.sink, resumed.offset)),
            Err(_) => {
                let _ = writer.discard_boxed(destination, &state).await;
            }
        },
        Some(Previous::Stale(state)) => {
            let _ = writer.discard_boxed(destination, &state).await;
        }
        None => {}
    }
    let sink = writer.create_boxed(destination, item.metadata().size).await?;
    Ok((sink, ByteSize(0)))
}

/// Journals a large write once its sink can describe how to continue it.
struct Checkpoint {
    pending: Option<InFlightWrite>,
    recorded: bool,
}

impl Checkpoint {
    fn new(route: &Route<'_>, item: &FileEntry, destination: &StoragePath, resumed: bool) -> Self {
        let large = item.metadata().size.is_some_and(|size| size >= RESUMABLE_SIZE);
        let pending = (route.resume.is_journaled() && large).then(|| InFlightWrite {
            source: item.path().clone(),
            target: destination.clone(),
            source_size: item.metadata().size.unwrap_or_default(),
            source_modified: item.metadata().modified,
            state: crate::ports::ResumeState(String::new()),
        });
        // A resumed write keeps its journal row until it finishes.
        Self { pending, recorded: resumed }
    }

    async fn record(&mut self, route: &Route<'_>, sink: &mut dyn WriteSink) {
        if self.recorded {
            return;
        }
        let Some(mut write) = self.pending.take() else { return };
        match sink.checkpoint() {
            Some(state) => {
                write.state = state;
                route.resume.write_started(write).await;
                self.recorded = true;
            }
            None => self.pending = Some(write),
        }
    }

    async fn finish(&self, route: &Route<'_>) {
        if self.recorded {
            route.resume.write_finished().await;
        }
    }
}

/// Renames when the provider can; otherwise copies and then deletes the source.
pub(crate) async fn move_item(
    handle: &MountHandle,
    entry: &FileEntry,
    target: &StoragePath,
    tracker: &Arc<ProgressTracker>,
    cancel: &CancelToken,
) -> Result<(), OperationError> {
    tracker.item_started(entry.name().into());
    let mover = handle.mover().map_err(OperationError::on(entry.path()))?;
    match mover.rename_boxed(entry.path(), target).await {
        Ok(()) => {
            tracker.item_finished();
            Ok(())
        }
        Err(VfsError::CrossDevice) => {
            let lister = handle.lister().map_err(OperationError::on(entry.path()))?;
            let tree = scan(&*lister, entry.clone(), cancel).await.map_err(OperationError::on(entry.path()))?;
            // The root item was already counted as one item when the move started.
            tracker.add_totals(tree.item_count().saturating_sub(1), tree.bytes);
            let route = Route { source: handle, dest: handle, same_mount: true, resume: &NO_RESUMPTION };
            move_across(&route, entry, &tree, target, tracker, cancel).await
        }
        Err(error) => Err(OperationError::on(entry.path())(error)),
    }
}

/// Copy, then delete the source. A failed copy is cleaned up, so the source
/// is never deleted unless the copy is complete.
pub(crate) async fn move_across(
    route: &Route<'_>,
    entry: &FileEntry,
    tree: &ScannedTree,
    target: &StoragePath,
    tracker: &Arc<ProgressTracker>,
    cancel: &CancelToken,
) -> Result<(), OperationError> {
    let on_source = OperationError::on(entry.path());
    let source_remover = route.source.remover().map_err(OperationError::on(entry.path()))?;
    if let Err(failure) = copy_tree(route, tree, target, tracker, cancel).await {
        if failure.root_created
            && let Ok(remover) = route.dest.remover()
        {
            // Our own partial copy; the source is untouched, so nothing is lost.
            let _ = remover.remove_boxed(target, CancelToken::never()).await;
        }
        return Err(failure.error);
    }
    source_remover.remove_boxed(entry.path(), cancel.clone()).await.map_err(on_source)
}
