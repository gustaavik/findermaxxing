use std::sync::Arc;

use domain::{Operation, OperationKind, PathError, PathSegment, Relocation, StoragePath};

use crate::operations::{OperationError, OperationHistory};
use crate::ports::MountResolver;

/// Renames one item in place. Never overwrites: a taken name is an error.
pub struct RenameEntry {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl RenameEntry {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history }
    }

    /// Returns the new location (unchanged if the name did not change).
    ///
    /// # Errors
    /// `Failed { AlreadyExists, .. }` for a taken name, or any provider error.
    pub async fn execute(&self, item: &StoragePath, new_name: PathSegment) -> Result<StoragePath, OperationError> {
        let parent = item.parent().ok_or(OperationError::on(item)(PathError::Root.into()))?;
        let target = parent.join(new_name);
        if &target == item {
            return Ok(target);
        }
        let mover =
            self.mounts.resolve(item.mount()).and_then(|mount| mount.mover()).map_err(OperationError::on(item))?;
        mover.rename_boxed(item, &target).await.map_err(OperationError::on(item))?;
        self.history.record(
            OperationKind::Rename,
            Operation::Moved(vec![Relocation { from: item.clone(), to: target.clone() }]),
        );
        Ok(target)
    }
}

impl std::fmt::Debug for RenameEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenameEntry").finish_non_exhaustive()
    }
}
