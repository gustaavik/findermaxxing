//! Executes prompt-free [`Action`]s (trash, restore, move-back). Used by the
//! trash use case and by undo/redo.

use std::sync::Arc;

use domain::{Action, ByteSize, Operation, Relocation, StoragePath, TrashTicket};

use super::context::JobContext;
use super::error::{OperationError, OperationReport};
use super::progress::ProgressTracker;
use super::transfer::move_item;
use crate::ports::MountResolver;

pub(crate) async fn execute(mounts: &dyn MountResolver, action: &Action, ctx: &JobContext) -> OperationReport {
    let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
    let steps = flatten(action);
    tracker.begin(steps.iter().map(step_items).sum(), ByteSize(0));
    let mut done = Vec::with_capacity(steps.len());
    for step in &steps {
        let (operation, result) = run_step(mounts, step, &tracker, ctx).await;
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
    tracker: &Arc<ProgressTracker>,
    ctx: &JobContext,
) -> StepOutcome {
    match step {
        Action::Trash(paths) => trash_all(mounts, paths, tracker, ctx).await,
        Action::Restore(tickets) => restore_all(mounts, tickets, tracker, ctx).await,
        Action::Move(moves) => move_all(mounts, moves, tracker, ctx).await,
        Action::Sequence(_) => (Operation::sequence(Vec::new()), Ok(())),
    }
}

async fn trash_all(
    mounts: &dyn MountResolver,
    paths: &[StoragePath],
    tracker: &ProgressTracker,
    ctx: &JobContext,
) -> StepOutcome {
    let mut tickets = Vec::with_capacity(paths.len());
    for path in paths {
        if ctx.cancel.is_cancelled() {
            return (Operation::Trashed(tickets), Err(OperationError::Cancelled));
        }
        tracker.item_started(path.name());
        let trashed = match mounts.resolve(path.mount()).and_then(|mount| mount.trasher()) {
            Ok(trasher) => trasher.trash_boxed(path).await,
            Err(error) => Err(error),
        };
        match trashed {
            Ok(ticket) => tickets.push(ticket),
            Err(error) => return (Operation::Trashed(tickets), Err(OperationError::on(path)(error))),
        }
        tracker.item_finished();
    }
    (Operation::Trashed(tickets), Ok(()))
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
    if relocation.from.mount() != relocation.to.mount() {
        return Err(OperationError::CrossMount);
    }
    let on_source = OperationError::on(&relocation.from);
    let handle = mounts.resolve(relocation.from.mount()).map_err(OperationError::on(&relocation.from))?;
    let lister = handle.lister().map_err(OperationError::on(&relocation.from))?;
    let entry = lister.stat_boxed(&relocation.from).await.map_err(on_source)?;
    move_item(&handle, &entry, &relocation.to, tracker, &ctx.cancel).await
}
