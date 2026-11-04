//! Renames many items in one go, including permutations (`a→b`, `b→a`) and
//! shift chains (`f 2→f 3`, `f 3→f 4`) that would collide if done one by one:
//! those are first moved to unique staging names, then to their targets.

use std::collections::HashSet;

use domain::{PathSegment, Relocation, StoragePath, VfsError};

use super::context::JobContext;
use super::error::OperationError;
use super::progress::ProgressTracker;
use crate::ports::{DynMover, MountResolver};

const STAGING_PREFIX: &str = ".findermaxxing-renaming";
/// Attempts to find an unused staging name before giving up.
const STAGING_ATTEMPTS: u32 = 100;

/// Whether `renames` needs staging because some target is another item's source.
pub(crate) fn needs_staging(renames: &[Relocation]) -> bool {
    let sources: HashSet<&StoragePath> = renames.iter().map(|r| &r.from).collect();
    renames.iter().any(|r| r.to != r.from && sources.contains(&r.to))
}

pub(crate) async fn rename_batch(
    mounts: &dyn MountResolver,
    renames: &[Relocation],
    tracker: &ProgressTracker,
    ctx: &JobContext,
) -> (Vec<Relocation>, Result<(), OperationError>) {
    let Some(first) = renames.first() else { return (Vec::new(), Ok(())) };
    if renames.iter().any(|r| r.from.mount() != first.from.mount() || r.to.mount() != first.from.mount()) {
        return (Vec::new(), Err(OperationError::CrossMount));
    }
    let mover = match mounts.resolve(first.from.mount()).and_then(|mount| mount.mover()) {
        Ok(mover) => mover,
        Err(error) => return (Vec::new(), Err(OperationError::on(&first.from)(error))),
    };
    if needs_staging(renames) {
        staged(&*mover, renames, tracker, ctx).await
    } else {
        direct(&*mover, renames, tracker, ctx).await
    }
}

async fn direct(
    mover: &dyn DynMover,
    renames: &[Relocation],
    tracker: &ProgressTracker,
    ctx: &JobContext,
) -> (Vec<Relocation>, Result<(), OperationError>) {
    let mut done = Vec::with_capacity(renames.len());
    for rename in renames {
        if ctx.cancel.is_cancelled() {
            return (done, Err(OperationError::Cancelled));
        }
        tracker.item_started(rename.from.name());
        if let Err(error) = mover.rename_boxed(&rename.from, &rename.to).await {
            return (done, Err(OperationError::on(&rename.from)(error)));
        }
        done.push(rename.clone());
        tracker.item_finished();
    }
    (done, Ok(()))
}

/// Not cancellable between the phases: stopping halfway would leave items
/// under staging names, so once phase one has started the batch completes.
async fn staged(
    mover: &dyn DynMover,
    renames: &[Relocation],
    tracker: &ProgressTracker,
    ctx: &JobContext,
) -> (Vec<Relocation>, Result<(), OperationError>) {
    if ctx.cancel.is_cancelled() {
        return (Vec::new(), Err(OperationError::Cancelled));
    }
    let mut staging: Vec<StoragePath> = Vec::with_capacity(renames.len());
    for (index, rename) in renames.iter().enumerate() {
        match stage(mover, &rename.from, index).await {
            Ok(temporary) => staging.push(temporary),
            Err(error) => {
                undo_staging(mover, renames, &staging).await;
                return (Vec::new(), Err(OperationError::on(&rename.from)(error)));
            }
        }
    }
    let mut done = Vec::with_capacity(renames.len());
    for (position, (rename, temporary)) in renames.iter().zip(&staging).enumerate() {
        tracker.item_started(rename.to.name());
        if let Err(error) = mover.rename_boxed(temporary, &rename.to).await {
            undo_staging(mover, &renames[position..], &staging[position..]).await;
            return (done, Err(OperationError::on(&rename.from)(error)));
        }
        done.push(rename.clone());
        tracker.item_finished();
    }
    (done, Ok(()))
}

async fn stage(mover: &dyn DynMover, item: &StoragePath, index: usize) -> Result<StoragePath, VfsError> {
    let parent = item.parent().ok_or(VfsError::NotFound)?;
    for attempt in 0..STAGING_ATTEMPTS {
        let temporary = parent.join(PathSegment::new(&format!("{STAGING_PREFIX}-{index}-{attempt}"))?);
        match mover.rename_boxed(item, &temporary).await {
            Ok(()) => return Ok(temporary),
            Err(VfsError::AlreadyExists) => {}
            Err(error) => return Err(error),
        }
    }
    Err(VfsError::AlreadyExists)
}

/// Best effort: puts staged items back under their original names.
async fn undo_staging(mover: &dyn DynMover, renames: &[Relocation], staging: &[StoragePath]) {
    for (rename, temporary) in renames.iter().zip(staging) {
        let _ = mover.rename_boxed(temporary, &rename.from).await;
    }
}
