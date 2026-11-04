use std::sync::Arc;

use application::operations::OperationHistory;
use application::ports::{FavoritesStore, LocalFileOpener, MountResolver, SavedSearchStore, VolumeLister};
use application::use_cases::{
    AccountServices, AddBucket, AddFavorite, AddS3Account, BulkRenameEntries, ConnectSavedAccounts, CopyEntries,
    CreateFolder, CreateShareLink, DeletePermanently, DeleteSavedSearch, GetItemProperties, ListBuckets, ListDirectory,
    ListFavorites, ListItemVersions, ListS3Accounts, ListSavedSearches, ListVolumes, MoveEntries, OpenEntry,
    PreparePreview, RedoOperation, RemoveFavorite, RemoveS3Account, RenameEntry, SaveSearch, SearchFiles,
    SetCostWarnings, TrashEntries, UndoOperation, UpdateItemProperties, WatchDirectory,
};
use domain::{CostPolicy, StoragePath};
use tokio::runtime::Handle;

/// Everything bridge objects need, built by the composition root and passed
/// down the QML tree explicitly (no globals): `main` → root window property →
/// each controller's `services`.
#[derive(Clone)]
pub struct AppServices {
    /// For asking what a location can do (watch, trash, metadata …).
    pub mounts: Arc<dyn MountResolver>,
    pub list_directory: Arc<ListDirectory>,
    pub watch_directory: Arc<WatchDirectory>,
    pub search: Arc<SearchFiles>,
    pub preview: Arc<PreparePreview>,
    pub saved_searches: SavedSearches,
    pub operations: FileOperations,
    pub places: Places,
    pub accounts: RemoteAccounts,
    pub item_details: ItemDetails,
    /// Runs every background task of the UI (sessions, jobs, previews).
    pub runtime: Handle,
    /// Where new windows open; bare paths typed by the user resolve on its mount.
    pub home: StoragePath,
}

impl std::fmt::Debug for AppServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppServices").field("home", &self.home).finish_non_exhaustive()
    }
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
    pub fn new(mounts: &Arc<dyn MountResolver>, opener: Arc<dyn LocalFileOpener>, costs: CostPolicy) -> Self {
        let history = Arc::new(OperationHistory::default());
        Self {
            copy: Arc::new(CopyEntries::new(mounts.clone(), history.clone()).with_cost_policy(costs)),
            move_entries: Arc::new(MoveEntries::new(mounts.clone(), history.clone()).with_cost_policy(costs)),
            rename: Arc::new(RenameEntry::new(mounts.clone(), history.clone())),
            bulk_rename: Arc::new(BulkRenameEntries::new(mounts.clone(), history.clone())),
            trash: Arc::new(TrashEntries::new(mounts.clone(), history.clone())),
            delete: Arc::new(DeletePermanently::new(mounts.clone())),
            create_folder: Arc::new(CreateFolder::new(mounts.clone(), history.clone())),
            open: Arc::new(OpenEntry::new(mounts.clone(), opener)),
            undo: Arc::new(UndoOperation::new(mounts.clone(), history.clone())),
            redo: Arc::new(RedoOperation::new(mounts.clone(), history.clone())),
            history,
        }
    }
}

/// Sidebar sources: favourites and mounted volumes.
#[derive(Debug, Clone)]
pub struct Places {
    pub favorites: Arc<ListFavorites>,
    pub add_favorite: Arc<AddFavorite>,
    pub remove_favorite: Arc<RemoveFavorite>,
    pub volumes: Arc<ListVolumes>,
}

impl Places {
    #[must_use]
    pub fn new(favorites: &Arc<dyn FavoritesStore>, volumes: Arc<dyn VolumeLister>) -> Self {
        Self {
            favorites: Arc::new(ListFavorites::new(favorites.clone())),
            add_favorite: Arc::new(AddFavorite::new(favorites.clone())),
            remove_favorite: Arc::new(RemoveFavorite::new(favorites.clone())),
            volumes: Arc::new(ListVolumes::new(volumes)),
        }
    }
}

/// S3 account use cases.
#[derive(Debug, Clone)]
pub struct RemoteAccounts {
    pub add: Arc<AddS3Account>,
    pub list: Arc<ListS3Accounts>,
    pub connect_saved: Arc<ConnectSavedAccounts>,
    pub remove: Arc<RemoveS3Account>,
    pub buckets: Arc<ListBuckets>,
    pub add_bucket: Arc<AddBucket>,
    pub cost_warnings: Arc<SetCostWarnings>,
}

impl RemoteAccounts {
    #[must_use]
    pub fn new(services: &AccountServices) -> Self {
        Self {
            add: Arc::new(AddS3Account::new(services.clone())),
            list: Arc::new(ListS3Accounts::new(services.store.clone())),
            connect_saved: Arc::new(ConnectSavedAccounts::new(services.clone())),
            remove: Arc::new(RemoveS3Account::new(services.clone())),
            buckets: Arc::new(ListBuckets::new(services.connector.clone(), services.store.clone())),
            add_bucket: Arc::new(AddBucket::new(services.store.clone())),
            cost_warnings: Arc::new(SetCostWarnings::new(services.store.clone())),
        }
    }
}

/// "Get Info" for items on providers with metadata, share links and versions.
#[derive(Debug, Clone)]
pub struct ItemDetails {
    pub properties: Arc<GetItemProperties>,
    pub update: Arc<UpdateItemProperties>,
    pub share_link: Arc<CreateShareLink>,
    pub versions: Arc<ListItemVersions>,
}

impl ItemDetails {
    #[must_use]
    pub fn new(mounts: &Arc<dyn MountResolver>) -> Self {
        Self {
            properties: Arc::new(GetItemProperties::new(mounts.clone())),
            update: Arc::new(UpdateItemProperties::new(mounts.clone())),
            share_link: Arc::new(CreateShareLink::new(mounts.clone())),
            versions: Arc::new(ListItemVersions::new(mounts.clone())),
        }
    }
}

/// `AppServices` over in-memory fakes, for presentation tests.
#[cfg(test)]
pub(crate) fn fake_services(fake: &application::testing::FakeVfs) -> AppServices {
    use application::ProviderRegistry;
    use application::testing::{
        FakeS3Connector, FixedVolumes, InMemoryAccounts, InMemoryFavorites, InMemoryKeychain, InMemorySavedSearches,
        InstantClock, RecordingOpener, StubArchiveInspector, StubTextRenderer,
    };
    use application::use_cases::CoalesceWindow;

    #[allow(clippy::unwrap_used, reason = "test helper")]
    let mounts: Arc<dyn MountResolver> =
        Arc::new(ProviderRegistry::default().with_provider(fake.clone().into_factory()).unwrap());
    let saved: Arc<dyn SavedSearchStore> = Arc::new(InMemorySavedSearches::default());
    let favorites: Arc<dyn FavoritesStore> = Arc::new(InMemoryFavorites::default());
    let accounts = AccountServices {
        store: Arc::new(InMemoryAccounts::default()),
        credentials: Arc::new(InMemoryKeychain::default()),
        connector: Arc::new(FakeS3Connector::default()),
    };
    AppServices {
        list_directory: Arc::new(ListDirectory::new(mounts.clone())),
        watch_directory: Arc::new(WatchDirectory::new(
            mounts.clone(),
            Arc::new(InstantClock),
            CoalesceWindow::default(),
        )),
        search: Arc::new(SearchFiles::new(mounts.clone())),
        preview: Arc::new(PreparePreview::new(
            mounts.clone(),
            Arc::new(StubTextRenderer),
            Arc::new(StubArchiveInspector),
        )),
        saved_searches: SavedSearches::new(&saved),
        operations: FileOperations::new(&mounts, Arc::new(RecordingOpener::default()), CostPolicy::default()),
        places: Places::new(&favorites, Arc::new(FixedVolumes::default())),
        accounts: RemoteAccounts::new(&accounts),
        item_details: ItemDetails::new(&mounts),
        mounts,
        runtime: Handle::current(),
        home: fake.path("/"),
    }
}
