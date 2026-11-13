//! Ports: small, role-based traits implemented by adapters.
//!
//! Each role has a static trait (`Lister`, native `async`/`impl Stream`) for
//! adapters to implement, and an object-safe `Dyn*` twin with a blanket impl
//! so the registry can store heterogeneous providers behind `Arc<dyn …>`.

mod accounts;
mod cache;
mod clock;
mod drives;
mod job;
mod journal;
mod lister;
mod mount;
mod mutation;
mod organize;
mod places;
mod preview;
mod remote_item;
mod saved_searches;
mod semantic;
mod servers;
mod stream;
mod terminal;
mod utilities;
mod watcher;

pub use accounts::{
    AccountStore, BucketInfo, CredentialError, CredentialKey, CredentialStore, NewS3Account, S3Connector, Secret,
};
pub use cache::{AccountMounts, CacheIndex, CacheJanitor, CacheRecord, PinStore, is_pinned};
pub use clock::Clock;
pub use drives::{DriveAccountStore, DriveConnector, NewDriveAccount, SignInError, SignedIn, UrlOpener};
pub use job::{ConflictResolver, CostConfirmer, JobObserver};
pub use journal::{InFlightWrite, InterruptedTransfer, JournalEntry, JournalId, JournaledKind, TransferJournal};
pub use lister::{DynLister, Lister};
pub use mount::{MountHandle, MountHandleBuilder, MountResolver, ProviderFactory};
pub use mutation::{
    Copier, DynCopier, DynFolderMaker, DynLauncher, DynMover, DynRemover, DynTrasher, FolderMaker, Launcher, Mover,
    ProgressCallback, Remover, Trasher,
};
pub use organize::{
    CloudModelConfig, CloudModelSettings, CloudModelStore, CloudProvider, ItemSummary, ModelError, ModelProposal,
    OrganizationModel, OrganizeRequest, ProposedGroup, ProposedRename, ProposedTags, TagEditor,
};
pub use places::{FavoritesStore, LocalFileOpener, Volume, VolumeLister};
pub use preview::{
    ArchiveInspector, ArchiveItem, ArchiveListing, DynMaterializer, LocalFile, Materializer, PreviewFailure,
    PreviewTheme, RenderedText, TextRenderer,
};
pub use remote_item::{ItemProperties, ItemVersion, MetadataEditor, Presigner, PropertyChanges, VersionLister};
pub use saved_searches::{NewSavedSearch, SavedSearchStore, StoreError};
pub use semantic::{
    AiSettings, AiSettingsStore, ContentExtractor, EmbedError, Embedding, ImageEmbedder, IndexHit, IndexObserver,
    IndexQuery, IndexedItem, IndexedPassage, ModelId, ModelInstaller, ModelPack, SemanticIndex, TextEmbedder,
};
pub use servers::{ConnectError, NewServerAccount, ServerAccountStore, ServerConnector};
pub use stream::{ByteStream, DynReader, DynWriter, Reader, ResumeState, ResumedSink, WriteSink, Writer, skip_bytes};
pub use terminal::{
    CellPoint, CursorShape, KeyModifiers, ScreenChanged, StyledRun, TerminalColor, TerminalError, TerminalKey,
    TerminalLauncher, TerminalScreen, TerminalSession, TerminalSize, TextStyle,
};
pub use utilities::{
    ArchiveBuilder, ArchiveError, ArchiveEvent, ArchiveExtractor, ContentHasher, ImageConverter, ImageError,
    ImageFingerprinter, ImageMetadata, PermissionEditor, RunningHash, SecureEraser, SkipReason, UtilityTools,
};
pub use watcher::{DynWatcher, Watcher};
