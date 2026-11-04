use std::sync::Arc;

use application::operations::OperationHistory;
use application::ports::MountResolver;
use application::use_cases::{
    CopyEntries, CreateFolder, DeletePermanently, ListDirectory, MoveEntries, OpenEntry, RedoOperation, RenameEntry,
    TrashEntries, UndoOperation, WatchDirectory,
};
use domain::StoragePath;
use tokio::runtime::Handle;

/// Everything bridge objects need, built by the composition root and passed
/// down the QML tree explicitly (no globals): `main` → root window property →
/// each controller's `services`.
#[derive(Debug, Clone)]
pub struct AppServices {
    pub(crate) list_directory: Arc<ListDirectory>,
    pub(crate) watch_directory: Arc<WatchDirectory>,
    pub(crate) operations: FileOperations,
    pub(crate) runtime: Handle,
    pub(crate) home: StoragePath,
}

impl AppServices {
    #[must_use]
    pub fn new(
        list_directory: Arc<ListDirectory>,
        watch_directory: Arc<WatchDirectory>,
        operations: FileOperations,
        runtime: Handle,
        home: StoragePath,
    ) -> Self {
        Self { list_directory, watch_directory, operations, runtime, home }
    }
}

/// The file-operation use cases of one window, sharing one undo history.
#[derive(Debug, Clone)]
pub struct FileOperations {
    pub copy: Arc<CopyEntries>,
    pub move_entries: Arc<MoveEntries>,
    pub rename: Arc<RenameEntry>,
    pub trash: Arc<TrashEntries>,
    pub delete: Arc<DeletePermanently>,
    pub create_folder: Arc<CreateFolder>,
    pub open: Arc<OpenEntry>,
    pub undo: Arc<UndoOperation>,
    pub redo: Arc<RedoOperation>,
    pub history: Arc<OperationHistory>,
}

impl FileOperations {
    /// Every use case over the same mounts, sharing one fresh history.
    #[must_use]
    pub fn new(mounts: &Arc<dyn MountResolver>) -> Self {
        let history = Arc::new(OperationHistory::default());
        Self {
            copy: Arc::new(CopyEntries::new(mounts.clone(), history.clone())),
            move_entries: Arc::new(MoveEntries::new(mounts.clone(), history.clone())),
            rename: Arc::new(RenameEntry::new(mounts.clone(), history.clone())),
            trash: Arc::new(TrashEntries::new(mounts.clone(), history.clone())),
            delete: Arc::new(DeletePermanently::new(mounts.clone())),
            create_folder: Arc::new(CreateFolder::new(mounts.clone(), history.clone())),
            open: Arc::new(OpenEntry::new(mounts.clone())),
            undo: Arc::new(UndoOperation::new(mounts.clone(), history.clone())),
            redo: Arc::new(RedoOperation::new(mounts.clone(), history.clone())),
            history,
        }
    }
}
