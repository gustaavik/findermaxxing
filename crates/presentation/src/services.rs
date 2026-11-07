use std::sync::Arc;

use application::operations::OperationHistory;
use application::ports::{
    CacheJanitor, FavoritesStore, LocalFileOpener, MountResolver, PinStore, SavedSearchStore, TransferJournal,
    VolumeLister,
};
use application::ports::{IndexObserver, ModelInstaller};
use application::ports::{TerminalLauncher, UtilityTools};
use application::use_cases::{
    AccountServices, AddBucket, AddFavorite, AddS3Account, BulkRenameEntries, ConnectSavedAccounts, CopyEntries,
    CreateFolder, CreateShareLink, DeletePermanently, DeleteSavedSearch, GetItemProperties, ListBuckets, ListDirectory,
    ListFavorites, ListItemVersions, ListS3Accounts, ListSavedSearches, ListVolumes, MoveEntries, OpenEntry,
    PreparePreview, RedoOperation, RemoveFavorite, RemoveS3Account, RenameEntry, SaveSearch, SearchFiles,
    SetCostWarnings, TrashEntries, UndoOperation, UpdateItemProperties, WatchDirectory,
};
use application::use_cases::{AddDrive, ConnectDrive, DriveServices, ListDrives, RemoveDrive};
use application::use_cases::{
    AddServer, ConnectServer, DiscardTransfer, ForgetLocalCopies, GetOfflineStatus, ListInterruptedTransfers,
    ListServers, MakeAvailableOffline, RemoveOfflineCopy, RemoveServer, ResumeTransfer, ServerServices,
};
use application::use_cases::{
    AiSearchServices, ChangeIndexScope, DisableAiSearch, EnableAiSearch, GetAiSettings, IndexerControl, PauseIndexing,
    SemanticSearch,
};
use application::use_cases::{
    ChangePermissions, CleanImageMetadata, CompressItems, ComputeChecksums, ConvertImages, ExtractArchive,
    FindDuplicates, FindPublishedChecksum, GetPermissions, MeasureDiskUsage, OpenTerminal, ReadImageMetadata,
    SecureDelete,
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
    pub servers: ServerAccounts,
    pub drives: CloudDrives,
    pub ai: AiSearch,
    pub offline: Offline,
    pub item_details: ItemDetails,
    pub utilities: Utilities,
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

/// The M7a utilities. The ones that change files share the window's undo
/// history with the file operations.
#[derive(Debug, Clone)]
pub struct Utilities {
    pub checksums: Arc<ComputeChecksums>,
    pub published_checksum: Arc<FindPublishedChecksum>,
    pub duplicates: Arc<FindDuplicates>,
    pub disk_usage: Arc<MeasureDiskUsage>,
    pub compress: Arc<CompressItems>,
    pub extract: Arc<ExtractArchive>,
    pub convert_images: Arc<ConvertImages>,
    pub read_metadata: Arc<ReadImageMetadata>,
    pub clean_metadata: Arc<CleanImageMetadata>,
    pub get_permissions: Arc<GetPermissions>,
    pub change_permissions: Arc<ChangePermissions>,
    pub secure_delete: Arc<SecureDelete>,
    pub terminal: Arc<OpenTerminal>,
}

impl Utilities {
    #[must_use]
    pub fn new(
        mounts: &Arc<dyn MountResolver>,
        operations: &FileOperations,
        tools: &UtilityTools,
        terminal: Arc<dyn TerminalLauncher>,
        home: std::path::PathBuf,
    ) -> Self {
        let history = &operations.history;
        Self {
            checksums: Arc::new(ComputeChecksums::new(mounts.clone(), tools.hasher.clone())),
            published_checksum: Arc::new(FindPublishedChecksum::new(mounts.clone())),
            duplicates: Arc::new(FindDuplicates::new(
                mounts.clone(),
                tools.hasher.clone(),
                tools.fingerprinter.clone(),
            )),
            disk_usage: Arc::new(MeasureDiskUsage::new(mounts.clone())),
            compress: Arc::new(CompressItems::new(mounts.clone(), history.clone(), tools.builder.clone())),
            extract: Arc::new(ExtractArchive::new(mounts.clone(), history.clone(), tools.extractor.clone())),
            convert_images: Arc::new(ConvertImages::new(mounts.clone(), history.clone(), tools.converter.clone())),
            read_metadata: Arc::new(ReadImageMetadata::new(mounts.clone(), tools.metadata.clone())),
            clean_metadata: Arc::new(CleanImageMetadata::new(mounts.clone(), history.clone(), tools.metadata.clone())),
            get_permissions: Arc::new(GetPermissions::new(mounts.clone())),
            change_permissions: Arc::new(ChangePermissions::new(mounts.clone(), history.clone())),
            secure_delete: Arc::new(SecureDelete::new(mounts.clone())),
            terminal: Arc::new(OpenTerminal::new(terminal, home)),
        }
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
    pub interrupted: Arc<ListInterruptedTransfers>,
    pub resume: Arc<ResumeTransfer>,
    pub discard: Arc<DiscardTransfer>,
    pub history: Arc<OperationHistory>,
}

impl FileOperations {
    /// Every use case over the same mounts, sharing one fresh history.
    /// Copies and moves are journaled so they can resume after a quit.
    #[must_use]
    pub fn new(
        mounts: &Arc<dyn MountResolver>,
        opener: Arc<dyn LocalFileOpener>,
        costs: CostPolicy,
        journal: &Arc<dyn TransferJournal>,
    ) -> Self {
        let history = Arc::new(OperationHistory::default());
        let copy = CopyEntries::new(mounts.clone(), history.clone()).with_cost_policy(costs);
        let move_entries = MoveEntries::new(mounts.clone(), history.clone()).with_cost_policy(costs);
        Self {
            copy: Arc::new(copy.with_journal(journal.clone())),
            move_entries: Arc::new(move_entries.with_journal(journal.clone())),
            rename: Arc::new(RenameEntry::new(mounts.clone(), history.clone())),
            bulk_rename: Arc::new(BulkRenameEntries::new(mounts.clone(), history.clone())),
            trash: Arc::new(TrashEntries::new(mounts.clone(), history.clone())),
            delete: Arc::new(DeletePermanently::new(mounts.clone())),
            create_folder: Arc::new(CreateFolder::new(mounts.clone(), history.clone())),
            open: Arc::new(OpenEntry::new(mounts.clone(), opener)),
            undo: Arc::new(UndoOperation::new(mounts.clone(), history.clone())),
            redo: Arc::new(RedoOperation::new(mounts.clone(), history.clone())),
            interrupted: Arc::new(ListInterruptedTransfers::new(journal.clone())),
            resume: Arc::new(ResumeTransfer::new(mounts.clone(), history.clone(), journal.clone())),
            discard: Arc::new(DiscardTransfer::new(mounts.clone(), journal.clone())),
            history,
        }
    }
}

/// SFTP, WebDAV and SMB account use cases.
#[derive(Debug, Clone)]
pub struct ServerAccounts {
    pub add: Arc<AddServer>,
    pub list: Arc<ListServers>,
    pub connect: Arc<ConnectServer>,
    pub remove: Arc<RemoveServer>,
}

impl ServerAccounts {
    #[must_use]
    pub fn new(services: &ServerServices) -> Self {
        Self {
            add: Arc::new(AddServer::new(services.clone())),
            list: Arc::new(ListServers::new(services.store.clone())),
            connect: Arc::new(ConnectServer::new(services.clone())),
            remove: Arc::new(RemoveServer::new(services.clone())),
        }
    }
}

/// Google Drive, Dropbox and OneDrive account use cases.
#[derive(Debug, Clone)]
pub struct CloudDrives {
    pub add: Arc<AddDrive>,
    pub list: Arc<ListDrives>,
    pub connect: Arc<ConnectDrive>,
    pub remove: Arc<RemoveDrive>,
}

impl CloudDrives {
    #[must_use]
    pub fn new(services: &DriveServices) -> Self {
        Self {
            add: Arc::new(AddDrive::new(services.clone())),
            list: Arc::new(ListDrives::new(services.store.clone())),
            connect: Arc::new(ConnectDrive::new(services.clone())),
            remove: Arc::new(RemoveDrive::new(services.clone())),
        }
    }
}

/// AI search: settings, model installs, searching, and the indexer's status.
#[derive(Clone)]
pub struct AiSearch {
    pub settings: Arc<GetAiSettings>,
    pub enable: Arc<EnableAiSearch>,
    pub disable: Arc<DisableAiSearch>,
    pub pause: Arc<PauseIndexing>,
    pub scope: Arc<ChangeIndexScope>,
    pub search: Arc<SemanticSearch>,
    pub models: Arc<dyn ModelInstaller>,
    pub control: IndexerControl,
    pub status: Arc<IndexStatusFeed>,
}

impl AiSearch {
    #[must_use]
    pub fn new(services: &AiSearchServices, search: SemanticSearch, status: Arc<IndexStatusFeed>) -> Self {
        Self {
            settings: Arc::new(GetAiSettings::new(services.clone())),
            enable: Arc::new(EnableAiSearch::new(services.clone())),
            disable: Arc::new(DisableAiSearch::new(services.clone())),
            pause: Arc::new(PauseIndexing::new(services.clone())),
            scope: Arc::new(ChangeIndexScope::new(services.clone())),
            search: Arc::new(search),
            models: services.installer.clone(),
            control: services.control.clone(),
            status,
        }
    }
}

impl std::fmt::Debug for AiSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiSearch").finish_non_exhaustive()
    }
}

type StatusListener = Box<dyn Fn(&domain::IndexStatus) + Send + Sync>;

/// The indexer's latest status, passed on to whoever listens (the AI search
/// controller, once QML exists). Created before the UI, by the composition root.
#[derive(Default)]
pub struct IndexStatusFeed {
    latest: std::sync::Mutex<domain::IndexStatus>,
    listeners: std::sync::Mutex<Vec<StatusListener>>,
}

impl IndexStatusFeed {
    #[must_use]
    pub fn latest(&self) -> domain::IndexStatus {
        self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn listen(&self, listener: StatusListener) {
        listener(&self.latest());
        self.listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(listener);
    }
}

impl IndexObserver for IndexStatusFeed {
    fn status(&self, status: &domain::IndexStatus) {
        *self.latest.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = status.clone();
        for listener in self.listeners.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter() {
            listener(status);
        }
    }
}

impl std::fmt::Debug for IndexStatusFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexStatusFeed").field("latest", &self.latest()).finish_non_exhaustive()
    }
}

/// Keeping remote items available offline, and forgetting a removed
/// account's local copies.
#[derive(Debug, Clone)]
pub struct Offline {
    pub make_available: Arc<MakeAvailableOffline>,
    pub remove: Arc<RemoveOfflineCopy>,
    pub status: Arc<GetOfflineStatus>,
    pub forget_account: Arc<ForgetLocalCopies>,
}

impl Offline {
    #[must_use]
    pub fn new(mounts: &Arc<dyn MountResolver>, pins: &Arc<dyn PinStore>, janitor: Arc<dyn CacheJanitor>) -> Self {
        Self {
            make_available: Arc::new(MakeAvailableOffline::new(mounts.clone(), pins.clone())),
            remove: Arc::new(RemoveOfflineCopy::new(pins.clone())),
            status: Arc::new(GetOfflineStatus::new(pins.clone())),
            forget_account: Arc::new(ForgetLocalCopies::new(janitor)),
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

/// Utilities over fakes: a toy hash, toy archives and images, an echo terminal.
#[cfg(test)]
fn fake_utilities(
    fake: &application::testing::FakeVfs,
    mounts: &Arc<dyn MountResolver>,
    operations: &FileOperations,
) -> Utilities {
    use application::testing::{FakeArchiver, FakeHasher, FakeImageTools, FakeTerminalLauncher};
    let archiver = Arc::new(FakeArchiver::new(fake.clone()));
    let images = Arc::new(FakeImageTools::new(fake.clone()));
    let tools = UtilityTools {
        hasher: Arc::new(FakeHasher),
        extractor: archiver.clone(),
        builder: archiver,
        converter: images.clone(),
        fingerprinter: images.clone(),
        metadata: images,
    };
    Utilities::new(mounts, operations, &tools, Arc::new(FakeTerminalLauncher::default()), "/".into())
}

/// AI search over fakes: a bag-of-words model and an in-memory index.
#[cfg(test)]
fn fake_ai_search(fake: &application::testing::FakeVfs, mounts: &Arc<dyn MountResolver>) -> AiSearch {
    let ai_settings = Arc::new(application::testing::InMemoryAiSettings::default());
    let installer = Arc::new(application::testing::FakeModelInstaller::default());
    let index = Arc::new(application::testing::InMemorySemanticIndex::default());
    let (_indexer, control) = application::use_cases::indexer(application::use_cases::IndexerParts {
        mounts: Arc::clone(mounts),
        extractor: Arc::new(application::testing::FakeExtractor::new(fake.clone())),
        text: Arc::new(application::testing::FakeTextEmbedder::default()),
        images: Arc::new(application::testing::FakeImageEmbedder),
        index: index.clone(),
        settings: ai_settings.clone(),
        installer: installer.clone(),
        clock: Arc::new(application::testing::InstantClock),
        observer: Arc::new(IndexStatusFeed::default()),
    });
    let ai_services = AiSearchServices {
        settings: ai_settings.clone(),
        installer: installer.clone(),
        index: index.clone(),
        control,
        default_scope: vec![fake.path("/")],
    };
    let search = SemanticSearch::new(
        Arc::new(application::testing::FakeTextEmbedder::default()),
        Arc::new(application::testing::FakeImageEmbedder),
        index,
        ai_settings,
        installer,
    );
    AiSearch::new(&ai_services, search, Arc::new(IndexStatusFeed::default()))
}

/// `AppServices` over in-memory fakes, for presentation tests.
#[cfg(test)]
pub(crate) fn fake_services(fake: &application::testing::FakeVfs) -> AppServices {
    use application::ProviderRegistry;
    use application::testing::{
        FakeS3Connector, FakeServerConnector, FixedVolumes, InMemoryAccounts, InMemoryFavorites, InMemoryJournal,
        InMemoryKeychain, InMemoryPins, InMemorySavedSearches, InMemoryServers, InstantClock, RecordingOpener,
        StubArchiveInspector, StubTextRenderer,
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
    let servers = ServerServices {
        store: Arc::new(InMemoryServers::default()),
        credentials: Arc::new(InMemoryKeychain::default()),
        connectors: vec![Arc::new(FakeServerConnector::new(domain::ServerProtocol::Sftp))],
    };
    let drives = DriveServices {
        store: Arc::new(application::testing::InMemoryDrives::default()),
        credentials: Arc::new(InMemoryKeychain::default()),
        connectors: vec![Arc::new(application::testing::FakeDriveConnector::new(domain::DriveProvider::Dropbox))],
    };
    let journal: Arc<dyn TransferJournal> = Arc::new(InMemoryJournal::default());
    let pins: Arc<dyn PinStore> = Arc::new(InMemoryPins::default());
    let operations =
        FileOperations::new(&mounts, Arc::new(RecordingOpener::default()), CostPolicy::default(), &journal);
    let utilities = fake_utilities(fake, &mounts, &operations);
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
        operations,
        utilities,
        places: Places::new(&favorites, Arc::new(FixedVolumes::default())),
        accounts: RemoteAccounts::new(&accounts),
        servers: ServerAccounts::new(&servers),
        drives: CloudDrives::new(&drives),
        ai: fake_ai_search(fake, &mounts),
        offline: Offline::new(&mounts, &pins, Arc::new(application::testing::RecordingJanitor::default())),
        item_details: ItemDetails::new(&mounts),
        mounts,
        runtime: Handle::current(),
        home: fake.path("/"),
    }
}
