use domain::{Operation, StoragePath, VfsError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperationError {
    #[error("{error} ({item})")]
    Failed { error: VfsError, item: StoragePath },
    #[error("the operation was cancelled")]
    Cancelled,
    #[error("the operation was stopped")]
    Stopped,
    #[error("a folder cannot be moved or copied into itself ({item})")]
    IntoItself { item: StoragePath },
    #[error("items on different storage locations can't be renamed together")]
    CrossMount,
    #[error("there is nothing to undo")]
    NothingToUndo,
    #[error("there is nothing to redo")]
    NothingToRedo,
    #[error("the app's settings couldn't be saved: {0}")]
    Store(String),
}

impl OperationError {
    /// Attributes a provider error to the item it happened on. Cancellation is
    /// not a failure of any item, so it maps to [`OperationError::Cancelled`].
    pub(crate) fn on(item: &StoragePath) -> impl FnOnce(VfsError) -> Self + '_ {
        move |error| match error {
            VfsError::Cancelled => Self::Cancelled,
            error => Self::Failed { error, item: item.clone() },
        }
    }
}

/// What a job actually did (always recorded, even when it failed part-way, so
/// the completed part stays undoable) and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationReport {
    pub done: Operation,
    pub result: Result<(), OperationError>,
}

impl OperationReport {
    pub(crate) fn new(done: Operation, result: Result<(), OperationError>) -> Self {
        Self { done, result }
    }
}
