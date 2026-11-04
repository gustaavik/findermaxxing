use std::sync::Arc;

use domain::{Action, OperationKind, StoragePath};

use crate::operations::executor::{self, NoTrash};
use crate::operations::{JobContext, OperationError, OperationHistory, OperationReport};
use crate::ports::MountResolver;

/// Moves items to the platform trash. Undo puts them back where they were.
pub struct TrashEntries {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl TrashEntries {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history }
    }

    pub async fn execute(&self, items: &[StoragePath], ctx: &JobContext) -> OperationReport {
        let report = executor::execute(&*self.mounts, &Action::Trash(items.to_vec()), NoTrash::Refuse, ctx).await;
        self.history.record(OperationKind::Trash, report.done.clone());
        report
    }
}

/// Deletes items for good. Deliberately not journaled: there is nothing to undo.
pub struct DeletePermanently {
    mounts: Arc<dyn MountResolver>,
}

impl DeletePermanently {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// # Errors
    /// The first failure; items before it are already gone.
    pub async fn execute(&self, items: &[StoragePath], ctx: &JobContext) -> Result<(), OperationError> {
        for item in items {
            if ctx.cancel.is_cancelled() {
                return Err(OperationError::Cancelled);
            }
            let remover = self
                .mounts
                .resolve(item.mount())
                .and_then(|mount| mount.remover())
                .map_err(OperationError::on(item))?;
            remover.remove_boxed(item, ctx.cancel.clone()).await.map_err(OperationError::on(item))?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for TrashEntries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrashEntries").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for DeletePermanently {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeletePermanently").finish_non_exhaustive()
    }
}
