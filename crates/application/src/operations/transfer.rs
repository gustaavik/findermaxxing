//! Copy and move of top-level items into one destination folder, with
//! conflict resolution, progress, cancellation and an exact undo record.

use std::sync::Arc;

use domain::{
    ByteSize, Conflict, ConflictChoice, ConflictMemory, EntryKind, FIRST_DUPLICATE_NUMBER, FileEntry, Operation,
    Relocation, StoragePath, TrashTicket, VfsError, numbered_name,
};

use super::context::JobContext;
use super::error::{OperationError, OperationReport};
use super::progress::ProgressTracker;
use super::scan::{ScannedTree, scan};
use crate::cancel::CancelToken;
use crate::ports::{MountHandle, MountResolver};

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
    ctx: &JobContext,
) -> OperationReport {
    let mut transfer = match Transfer::new(mounts, mode, sources, destination, ctx) {
        Ok(transfer) => transfer,
        Err(error) => return OperationReport::new(Operation::sequence(Vec::new()), Err(error)),
    };
    let result = transfer.execute(sources).await;
    OperationReport::new(transfer.finish(), result)
}

struct Prepared {
    entry: FileEntry,
    /// Present for copies, which need every descendant up front for progress totals.
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
    handle: MountHandle,
    destination: &'a StoragePath,
    ctx: &'a JobContext,
    tracker: Arc<ProgressTracker>,
    memory: ConflictMemory,
    replaced: Vec<TrashTicket>,
    relocations: Vec<Relocation>,
}

impl<'a> Transfer<'a> {
    fn new(
        mounts: &dyn MountResolver,
        mode: TransferMode,
        sources: &[StoragePath],
        destination: &'a StoragePath,
        ctx: &'a JobContext,
    ) -> Result<Self, OperationError> {
        if sources.iter().any(|source| source.mount() != destination.mount()) {
            return Err(OperationError::CrossMount);
        }
        let handle = mounts.resolve(destination.mount()).map_err(OperationError::on(destination))?;
        Ok(Self {
            mode,
            handle,
            destination,
            ctx,
            tracker: ProgressTracker::new(Arc::clone(&ctx.observer)),
            memory: ConflictMemory::default(),
            replaced: Vec::new(),
            relocations: Vec::new(),
        })
    }

    fn finish(self) -> Operation {
        let transferred = match self.mode {
            TransferMode::Copy => Operation::Copied(self.relocations),
            TransferMode::Move => Operation::Moved(self.relocations),
        };
        Operation::sequence(vec![Operation::Trashed(self.replaced), transferred])
    }

    async fn execute(&mut self, sources: &[StoragePath]) -> Result<(), OperationError> {
        for item in self.prepare(sources).await? {
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
        let lister = self.handle.lister().map_err(OperationError::on(self.destination))?;
        let mut prepared = Vec::with_capacity(sources.len());
        for source in sources {
            let entry = lister.stat_boxed(source).await.map_err(OperationError::on(source))?;
            if entry.kind() == EntryKind::Directory && self.destination.is_within(source) {
                return Err(OperationError::IntoItself { item: source.clone() });
            }
            let tree = match self.mode {
                TransferMode::Copy => {
                    Some(scan(&*lister, entry.clone(), &self.ctx.cancel).await.map_err(OperationError::on(source))?)
                }
                TransferMode::Move => None,
            };
            prepared.push(Prepared { entry, tree });
        }
        let items = prepared.iter().map(Prepared::items).sum();
        let bytes = ByteSize(prepared.iter().map(|p| p.bytes().0).sum());
        self.tracker.begin(items, bytes);
        Ok(prepared)
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
            Some(existing) => self.resolve_conflict(entry, existing).await,
        }
    }

    async fn existing(&self, path: &StoragePath) -> Result<Option<FileEntry>, OperationError> {
        let lister = self.handle.lister().map_err(OperationError::on(path))?;
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
                let conflict = Conflict { incoming: entry.clone(), existing: existing.clone() };
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

    /// The replaced item goes to the trash, not oblivion, so undo can bring it back.
    async fn replace(&mut self, entry: &FileEntry, existing: &FileEntry) -> Result<StoragePath, OperationError> {
        if entry.path().is_within(existing.path()) {
            return Err(OperationError::IntoItself { item: existing.path().clone() });
        }
        let on_existing = OperationError::on(existing.path());
        let trasher = self.handle.trasher().map_err(OperationError::on(existing.path()))?;
        let ticket = trasher.trash_boxed(existing.path()).await.map_err(on_existing)?;
        self.replaced.push(ticket);
        Ok(existing.path().clone())
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
        let cancel = &self.ctx.cancel;
        let result = match &item.tree {
            Some(tree) => copy_tree(&self.handle, tree, &target, &self.tracker, cancel).await.map_err(|failure| {
                // A partial copy is still recorded so that undo cleans it up.
                if failure.root_created {
                    self.relocations.push(relocation.clone());
                }
                failure.error
            }),
            None => move_item(&self.handle, &item.entry, &target, &self.tracker, cancel).await,
        };
        result.map(|()| self.relocations.push(relocation))
    }
}

pub(crate) struct TreeFailure {
    pub root_created: bool,
    pub error: OperationError,
}

/// Recreates `tree` at `target`: folders via `FolderMaker`, everything else via `Copier`.
pub(crate) async fn copy_tree(
    handle: &MountHandle,
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
        let result = if item.kind() == EntryKind::Directory {
            match handle.folder_maker() {
                Ok(maker) => maker.create_folder_boxed(&destination).await.map(drop),
                Err(error) => Err(error),
            }
        } else {
            match handle.copier() {
                Ok(copier) => copier
                    .copy_item_boxed(item.path(), &destination, tracker.byte_callback(), cancel.clone())
                    .await
                    .map(drop),
                Err(error) => Err(error),
            }
        };
        result.map_err(fail)?;
        root_created = true;
        tracker.item_finished();
    }
    Ok(())
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
        Err(VfsError::CrossDevice) => move_by_copying(handle, entry, target, tracker, cancel).await,
        Err(error) => Err(OperationError::on(entry.path())(error)),
    }
}

async fn move_by_copying(
    handle: &MountHandle,
    entry: &FileEntry,
    target: &StoragePath,
    tracker: &Arc<ProgressTracker>,
    cancel: &CancelToken,
) -> Result<(), OperationError> {
    let on_source = OperationError::on(entry.path());
    let lister = handle.lister().map_err(OperationError::on(entry.path()))?;
    let remover = handle.remover().map_err(OperationError::on(entry.path()))?;
    let tree = scan(&*lister, entry.clone(), cancel).await.map_err(OperationError::on(entry.path()))?;
    // The root item was already counted as one item when the move started.
    tracker.add_totals(tree.item_count().saturating_sub(1), tree.bytes);
    if let Err(failure) = copy_tree(handle, &tree, target, tracker, cancel).await {
        if failure.root_created {
            // Our own partial copy; the source is untouched, so nothing is lost.
            let _ = remover.remove_boxed(target, CancelToken::never()).await;
        }
        return Err(failure.error);
    }
    remover.remove_boxed(entry.path(), cancel.clone()).await.map_err(on_source)
}
