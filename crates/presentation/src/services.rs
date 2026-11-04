use std::sync::Arc;

use application::operations::OperationHistory;
use application::ports::{MountResolver, SavedSearchStore};
use application::use_cases::{
    BulkRenameEntries, CopyEntries, CreateFolder, DeletePermanently, DeleteSavedSearch, ListDirectory,
    ListSavedSearches, MoveEntries, OpenEntry, PreparePreview, RedoOperation, RenameEntry, SaveSearch, SearchFiles,
    TrashEntries, UndoOperation, WatchDirectory,
};
use domain::StoragePath;
use tokio::runtime::Handle;

/// Everything bridge objects need, built by the composition root and passed
/// down the QML tree explicitly (no globals): `main` → root window property →
/// each controller's `services`.
#[derive(Debug, Clone)]
pub struct AppServices {
    pub list_directory: Arc<ListDirectory>,
    pub watch_directory: Arc<WatchDirectory>,
    pub search: Arc<SearchFiles>,
    pub preview: Arc<PreparePreview>,
    pub saved_searches: SavedSearches,
    pub operations: FileOperations,
    /// Runs every background task of the UI (sessions, jobs, previews).
    pub runtime: Handle,
    /// Where new windows open; bare paths typed by the user resolve on its mount.
    pub home: StoragePath,
}

/// Saved-search use cases over one store.
#[derive(Debug, Clone)]
pub struct SavedSearches {
    pub save: Arc<SaveSearch>,
    pub list: Arc<ListSavedSearches>,
    pub delete: Arc<DeleteSavedSearch>,
}

impl SavedSearches {
    #[must_use]
    pub fn new(store: &Arc<dyn SavedSearchStore>) -> Self {
        Self {
            save: Arc::new(SaveSearch::new(store.clone())),
            list: Arc::new(ListSavedSearches::new(store.clone())),
            delete: Arc::new(DeleteSavedSearch::new(store.clone())),
        }
    }
}

/// The file-operation use cases of one window, sharing one undo history.
#[derive(Debug, Clone)]
pub struct FileOperations {
    pub copy: Arc<CopyEntries>,
    pub move_entries: Arc<MoveEntries>,
    pub rename: Arc<RenameEntry>,
    pub bulk_rename: Arc<BulkRenameEntries>,
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
            bulk_rename: Arc::new(BulkRenameEntries::new(mounts.clone(), history.clone())),
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
