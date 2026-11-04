use std::sync::Arc;

use domain::{
    EntryKind, FIRST_DUPLICATE_NUMBER, Operation, OperationKind, PathSegment, StoragePath, VfsError, numbered_name,
};

use crate::operations::{OperationError, OperationHistory};
use crate::ports::MountResolver;

/// Creates a folder named `base_name`, or `base_name 2`, `3`, … if taken.
pub struct CreateFolder {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl CreateFolder {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history }
    }

    /// # Errors
    /// Provider errors other than a taken name.
    pub async fn execute(&self, parent: &StoragePath, base_name: &PathSegment) -> Result<StoragePath, OperationError> {
        let maker = self
            .mounts
            .resolve(parent.mount())
            .and_then(|mount| mount.folder_maker())
            .map_err(OperationError::on(parent))?;
        let mut candidate = parent.join(base_name.clone());
        let mut number = FIRST_DUPLICATE_NUMBER;
        loop {
            match maker.create_folder_boxed(&candidate).await {
                Ok(_) => break,
                Err(VfsError::AlreadyExists) => {
                    let name = numbered_name(base_name, EntryKind::Directory, number)
                        .map_err(|error| OperationError::on(parent)(error.into()))?;
                    candidate = parent.join(name);
                    number = number.saturating_add(1);
                }
                Err(error) => return Err(OperationError::on(&candidate)(error)),
            }
        }
        self.history.record(OperationKind::CreateFolder, Operation::CreatedFolder(candidate.clone()));
        Ok(candidate)
    }
}

impl std::fmt::Debug for CreateFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateFolder").finish_non_exhaustive()
    }
}
