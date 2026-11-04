use std::sync::Arc;

use crate::operations::{HistoryEntry, JobContext, OperationError, OperationHistory, OperationReport, executor};
use crate::ports::MountResolver;

/// Reverts the most recent operation. What the revert did becomes the redo entry.
pub struct UndoOperation {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl UndoOperation {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history }
    }

    /// Never prompts: undo must not be able to overwrite anything.
    ///
    /// # Errors
    /// [`OperationError::NothingToUndo`] when the history is empty.
    pub async fn execute(&self, ctx: &JobContext) -> Result<OperationReport, OperationError> {
        let entry = self.history.take_undo().ok_or(OperationError::NothingToUndo)?;
        let report = executor::execute(&*self.mounts, &entry.operation.inverse(), ctx).await;
        if report.done.is_empty() {
            // Nothing changed (e.g. the item is gone); keep the entry so the user can retry.
            self.history.push_undo(entry);
        } else {
            self.history.push_redo(HistoryEntry { kind: entry.kind, operation: report.done.clone() });
        }
        Ok(report)
    }
}

/// Re-applies the most recently undone operation.
pub struct RedoOperation {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl RedoOperation {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history }
    }

    /// # Errors
    /// [`OperationError::NothingToRedo`] when nothing was undone.
    pub async fn execute(&self, ctx: &JobContext) -> Result<OperationReport, OperationError> {
        let entry = self.history.take_redo().ok_or(OperationError::NothingToRedo)?;
        let report = executor::execute(&*self.mounts, &entry.operation.inverse(), ctx).await;
        if report.done.is_empty() {
            self.history.push_redo(entry);
        } else {
            self.history.push_undo(HistoryEntry { kind: entry.kind, operation: report.done.clone() });
        }
        Ok(report)
    }
}

impl std::fmt::Debug for UndoOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UndoOperation").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for RedoOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedoOperation").finish_non_exhaustive()
    }
}
