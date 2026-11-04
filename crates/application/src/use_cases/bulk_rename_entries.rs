use std::sync::Arc;

use domain::{ByteSize, Operation, OperationKind, PlannedRename, Relocation, RenameOutcome};

use crate::operations::progress::ProgressTracker;
use crate::operations::relocate::rename_batch;
use crate::operations::{JobContext, OperationHistory, OperationReport};
use crate::ports::MountResolver;

/// Applies the renames of a bulk-rename plan that are marked `Rename`;
/// unchanged and problematic items are left alone. Undoable as one step.
pub struct BulkRenameEntries {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl BulkRenameEntries {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history }
    }

    pub async fn execute(&self, plan: &[PlannedRename], ctx: &JobContext) -> OperationReport {
        let renames: Vec<Relocation> = plan
            .iter()
            .filter_map(|planned| match &planned.outcome {
                RenameOutcome::Rename(name) => {
                    let to = planned.item.parent()?.join(name.clone());
                    Some(Relocation { from: planned.item.clone(), to })
                }
                RenameOutcome::Unchanged | RenameOutcome::Problem(_) => None,
            })
            .collect();
        let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
        tracker.begin(renames.len() as u64, ByteSize(0));
        let (done, result) = rename_batch(&*self.mounts, &renames, &tracker, ctx).await;
        let operation = Operation::Moved(done);
        self.history.record(OperationKind::Rename, operation.clone());
        OperationReport { done: operation, result }
    }
}

impl std::fmt::Debug for BulkRenameEntries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BulkRenameEntries").finish_non_exhaustive()
    }
}
