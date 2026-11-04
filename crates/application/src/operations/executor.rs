//! Executes prompt-free [`Action`]s (trash, restore, move-back). Used by the
//! trash use case and by undo/redo.

use std::sync::Arc;

use domain::{Action, ByteSize, Operation, Relocation, StoragePath, TrashTicket};

use super::context::JobContext;
use super::error::{OperationError, OperationReport};
use super::progress::ProgressTracker;
use super::relocate::{needs_staging, rename_batch};
use super::scan::scan;
use super::transfer::{Route, move_across, move_item};
use crate::ports::MountResolver;

/// What "trash" means on a location without a Trash (S3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoTrash {
    /// User-initiated trashing: refuse (`Unsupported(Trash)`); the UI then
    /// offers an explicit permanent delete.
    Refuse,
    /// Undoing a copy or a new folder: delete what that operation created.
    /// The originals of a copy still exist, so nothing the user had is lost.
    DeleteCreated,
}

pub(crate) async fn execute(
    mounts: &dyn MountResolver,
    action: &Action,
    no_trash: NoTrash,
    ctx: &JobContext,
) -> OperationReport {
    let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
    let steps = flatten(action);
    tracker.begin(steps.iter().map(step_items).sum(), ByteSize(0));
    let mut done = Vec::with_capacity(steps.len());
    for step in &steps {
        let (operation, result) = run_step(mounts, step, no_trash, &tracker, ctx).await;
        done.push(operation);
        if let Err(error) = result {
            return OperationReport::new(Operation::sequence(done), Err(error));
        }
    }
    OperationReport::new(Operation::sequence(done), Ok(()))
}

fn flatten(action: &Action) -> Vec<Action> {
    match action {
        Action::Sequence(parts) => parts.iter().flat_map(flatten).collect(),
        simple => vec![simple.clone()],
    }
}

fn step_items(step: &Action) -> u64 {
    let count = match step {
        Action::Trash(paths) => paths.len(),
        Action::Restore(tickets) => tickets.len(),
        Action::Move(moves) => moves.len(),
        Action::Sequence(parts) => parts.len(),
    };
    count as u64
}

type StepOutcome = (Operation, Result<(), OperationError>);

async fn run_step(
    mounts: &dyn MountResolver,
    step: &Action,
    no_trash: NoTrash,
    tracker: &Arc<ProgressTracker>,
    ctx: &JobContext,
) -> StepOutcome {
    match step {
        Action::Trash(paths) => trash_all(mounts, paths, no_trash, tracker, ctx).await,
        Action::Restore(tickets) => restore_all(mounts, tickets, tracker, ctx).await,
        Action::Move(moves) => move_all(mounts, moves, tracker, ctx).await,
        Action::Sequence(_) => (Operation::sequence(Vec::new()), Ok(())),
    }
}

enum Disposal {
    Trashed(TrashTicket),
    Deleted,
}

async fn trash_all(
    mounts: &dyn MountResolver,
    paths: &[StoragePath],
    no_trash: NoTrash,
    tracker: &ProgressTracker,
    ctx: &JobContext,
) -> StepOutcome {
    let mut tickets = Vec::with_capacity(paths.len());
    let mut deleted = Vec::new();
    let outcome =
        |tickets, deleted| Operation::sequence(vec![Operation::Trashed(tickets), Operation::Deleted(deleted)]);
    for path in paths {
        if ctx.cancel.is_cancelled() {
            return (outcome(tickets, deleted), Err(OperationError::Cancelled));
        }
        tracker.item_started(path.name());
        match dispose(mounts, path, no_trash, ctx).await {
            Ok(Disposal::Trashed(ticket)) => tickets.push(ticket),
            Ok(Disposal::Deleted) => deleted.push(path.clone()),
            Err(error) => return (outcome(tickets, deleted), Err(OperationError::on(path)(error))),
        }
        tracker.item_finished();
    }
    (outcome(tickets, deleted), Ok(()))
}

async fn dispose(
    mounts: &dyn MountResolver,
    path: &StoragePath,
    no_trash: NoTrash,
    ctx: &JobContext,
) -> Result<Disposal, domain::VfsError> {
    let mount = mounts.resolve(path.mount())?;
    match (mount.trasher(), no_trash) {
        (Ok(trasher), _) => trasher.trash_boxed(path).await.map(Disposal::Trashed),
        (Err(error), NoTrash::Refuse) => Err(error),
        (Err(_), NoTrash::DeleteCreated) => {
            mount.remover()?.remove_boxed(path, ctx.cancel.clone()).await.map(|()| Disposal::Deleted)
        }
    }
}

async fn restore_all(
    mounts: &dyn MountResolver,
    tickets: &[TrashTicket],
    tracker: &ProgressTracker,
    ctx: &JobContext,
) -> StepOutcome {
    let mut restored = Vec::with_capacity(tickets.len());
    for ticket in tickets {
        if ctx.cancel.is_cancelled() {
            return (Operation::Restored(restored), Err(OperationError::Cancelled));
        }
        tracker.item_started(ticket.original.name());
        let result = match mounts.resolve(ticket.original.mount()).and_then(|mount| mount.trasher()) {
            Ok(trasher) => trasher.restore_boxed(ticket).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            return (Operation::Restored(restored), Err(OperationError::on(&ticket.original)(error)));
        }
        restored.push(ticket.clone());
        tracker.item_finished();
    }
    (Operation::Restored(restored), Ok(()))
}

async fn move_all(
    mounts: &dyn MountResolver,
    moves: &[Relocation],
    tracker: &Arc<ProgressTracker>,
    ctx: &JobContext,
) -> StepOutcome {
    if needs_staging(moves) {
        // E.g. undoing a swap from a bulk rename.
        let (moved, result) = rename_batch(mounts, moves, tracker, ctx).await;
        return (Operation::Moved(moved), result);
    }
    let mut moved = Vec::with_capacity(moves.len());
    for relocation in moves {
        if ctx.cancel.is_cancelled() {
            return (Operation::Moved(moved), Err(OperationError::Cancelled));
        }
        if let Err(error) = move_one(mounts, relocation, tracker, ctx).await {
            return (Operation::Moved(moved), Err(error));
        }
        moved.push(relocation.clone());
    }
    (Operation::Moved(moved), Ok(()))
}

async fn move_one(
    mounts: &dyn MountResolver,
    relocation: &Relocation,
    tracker: &Arc<ProgressTracker>,
    ctx: &JobContext,
) -> Result<(), OperationError> {
    let from = &relocation.from;
    let source = mounts.resolve(from.mount()).map_err(OperationError::on(from))?;
    let lister = source.lister().map_err(OperationError::on(from))?;
    let entry = lister.stat_boxed(from).await.map_err(OperationError::on(from))?;
    if from.mount() == relocation.to.mount() {
        return move_item(&source, &entry, &relocation.to, tracker, &ctx.cancel).await;
    }
    // E.g. undoing a move from a local folder into a bucket.
    let dest = mounts.resolve(relocation.to.mount()).map_err(OperationError::on(&relocation.to))?;
    let tree = scan(&*lister, entry.clone(), &ctx.cancel).await.map_err(OperationError::on(from))?;
    tracker.add_totals(tree.item_count().saturating_sub(1), tree.bytes);
    let route = Route { source: &source, dest: &dest, same_mount: false };
    move_across(&route, &entry, &tree, &relocation.to, tracker, &ctx.cancel).await
}
