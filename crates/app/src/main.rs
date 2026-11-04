//! Composition root. Builds the runtime, registers providers, constructs use
//! cases with their ports, and hands the result to the presentation layer.
//! No other crate names concrete adapters.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use adapter_file_cache::{CachingResolver, FileCache};
use adapter_keyring::KeyringStore;
use adapter_local_fs::{LocalFs, LocalFsFactory, LocalOpener, LocalTrash, LocalVolumes, storage_path_from_native};
use adapter_preview::{LocalArchiveInspector, SyntectTextRenderer};
use adapter_s3::S3Service;
use adapter_sftp::SftpService;
use adapter_smb::SmbService;
use adapter_sqlite_store::SqliteStore;
use adapter_tokio_runtime::TokioClock;
use adapter_webdav::WebDavService;
use application::ProviderRegistry;
use application::ports::{
    CacheIndex, CredentialStore, FavoritesStore, MountResolver, PinStore, SavedSearchStore, ServerConnector,
    StoreError, TransferJournal,
};
use application::use_cases::{
    AccountServices, CoalesceWindow, ListDirectory, PreparePreview, SearchFiles, ServerServices, WatchDirectory,
};
use domain::{CachePolicy, CostPolicy};
use presentation::{
    AppServices, FileOperations, ItemDetails, Offline, Places, RemoteAccounts, SavedSearches, ServerAccounts,
};
use tokio::runtime::Handle;

const HOME_VARIABLE: &str = "HOME";
const FALLBACK_HOME: &str = "/";
const RUNTIME_THREAD_NAME: &str = "findermaxxing-worker";
const APP_FOLDER: &str = "FinderMaxxing";
const METADATA_FILE: &str = "metadata.sqlite3";
const FILE_CACHE_FOLDER: &str = "files";
const KNOWN_HOSTS_FILE: &str = "known_hosts";
#[cfg(target_os = "macos")]
const PLATFORM_DATA_HOME: &str = "Library/Application Support";
#[cfg(not(target_os = "macos"))]
const PLATFORM_DATA_HOME: &str = ".local/share";
#[cfg(target_os = "macos")]
const PLATFORM_CACHE_HOME: &str = "Library/Caches";
#[cfg(not(target_os = "macos"))]
const PLATFORM_CACHE_HOME: &str = ".cache";
const DATA_HOME_VARIABLE: &str = "XDG_DATA_HOME";
const CACHE_HOME_VARIABLE: &str = "XDG_CACHE_HOME";

#[derive(Debug, thiserror::Error)]
enum StartupError {
    #[error("could not start the async runtime: {0}")]
    Runtime(#[from] std::io::Error),
    #[error("could not register storage providers: {0}")]
    Registry(#[from] application::RegistryError),
    #[error("invalid built-in path: {0}")]
    Path(#[from] domain::PathError),
    #[error("could not resolve the home folder: {0}")]
    Home(#[from] domain::VfsError),
    #[error("no metadata store, not even in memory: {0}")]
    Store(#[from] StoreError),
}

fn main() -> ExitCode {
    match start() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX)),
        Err(error) => {
            eprintln!("FinderMaxxing failed to start: {error}");
            ExitCode::FAILURE
        }
    }
}

fn start() -> Result<i32, StartupError> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name(RUNTIME_THREAD_NAME).build()?;
    let handle = runtime.handle().clone();
    let store = metadata_store(&handle)?;
    let providers = Providers::new(&handle)?;
    let cache = file_cache(&handle, &store);
    let mounts = build_registry(&handle, &providers, cache.clone())?;
    let saved: Arc<dyn SavedSearchStore> = store.clone();
    let favorites: Arc<dyn FavoritesStore> = store.clone();
    let pins: Arc<dyn PinStore> = store.clone();
    let journal: Arc<dyn TransferJournal> = store.clone();
    let credentials: Arc<dyn CredentialStore> = Arc::new(KeyringStore::new(handle.clone()));
    let servers = ServerServices {
        store: store.clone(),
        credentials: credentials.clone(),
        connectors: providers.server_connectors(),
    };
    let accounts = AccountServices { store, credentials, connector: Arc::new(providers.s3.clone()) };
    let services = AppServices {
        list_directory: Arc::new(ListDirectory::new(mounts.clone())),
        watch_directory: Arc::new(WatchDirectory::new(mounts.clone(), Arc::new(TokioClock), CoalesceWindow::default())),
        search: Arc::new(SearchFiles::new(mounts.clone())),
        preview: Arc::new(PreparePreview::new(
            mounts.clone(),
            Arc::new(SyntectTextRenderer::new(handle.clone())),
            Arc::new(LocalArchiveInspector::new(handle.clone())),
        )),
        saved_searches: SavedSearches::new(&saved),
        operations: FileOperations::new(
            &mounts,
            Arc::new(LocalOpener::new(handle.clone())),
            CostPolicy::default(),
            &journal,
        ),
        places: Places::new(&favorites, Arc::new(LocalVolumes::new(handle.clone()))),
        accounts: RemoteAccounts::new(&accounts),
        servers: ServerAccounts::new(&servers),
        offline: Offline::new(&mounts, &pins, Arc::new(cache)),
        item_details: ItemDetails::new(&mounts),
        mounts,
        runtime: handle,
        home: storage_path_from_native(&home_dir())?,
    };
    let exit_code = presentation::run(services);
    // Background tasks are cancelled with their sessions; don't wait on stragglers.
    runtime.shutdown_background();
    Ok(exit_code)
}

/// The remote providers; each is both a mount factory and a connector.
struct Providers {
    s3: S3Service,
    sftp: SftpService,
    webdav: WebDavService,
    smb: SmbService,
}

impl Providers {
    fn new(runtime: &Handle) -> Result<Self, StartupError> {
        let known_hosts = platform_dir(DATA_HOME_VARIABLE, PLATFORM_DATA_HOME).join(KNOWN_HOSTS_FILE);
        Ok(Self {
            s3: S3Service::new(runtime.clone())?,
            sftp: SftpService::new(runtime.clone(), known_hosts)?,
            webdav: WebDavService::new(runtime.clone())?,
            smb: SmbService::new(runtime.clone())?,
        })
    }

    fn server_connectors(&self) -> Vec<Arc<dyn ServerConnector>> {
        vec![Arc::new(self.sftp.clone()), Arc::new(self.webdav.clone()), Arc::new(self.smb.clone())]
    }
}

/// Every provider is registered here and nowhere else (Open/Closed). Remote
/// mounts get the local cache and offline access by decoration (ADR 0020).
fn build_registry(
    runtime: &Handle,
    providers: &Providers,
    cache: FileCache,
) -> Result<Arc<dyn MountResolver>, StartupError> {
    let local = LocalFsFactory::new(LocalFs::new(runtime.clone()), LocalTrash::platform_default(runtime.clone()))?;
    let registry = ProviderRegistry::default()
        .with_provider(Arc::new(local))?
        .with_provider(Arc::new(providers.s3.clone()))?
        .with_provider(Arc::new(providers.sftp.clone()))?
        .with_provider(Arc::new(providers.webdav.clone()))?
        .with_provider(Arc::new(providers.smb.clone()))?;
    Ok(Arc::new(CachingResolver::new(Arc::new(registry), cache)))
}

/// Downloaded copies of remote files, size-limited (ADR 0020).
fn file_cache(runtime: &Handle, store: &Arc<SqliteStore>) -> FileCache {
    let index: Arc<dyn CacheIndex> = store.clone();
    let pins: Arc<dyn PinStore> = store.clone();
    FileCache::new(
        platform_dir(CACHE_HOME_VARIABLE, PLATFORM_CACHE_HOME).join(FILE_CACHE_FOLDER),
        index,
        pins,
        CachePolicy::default(),
        runtime.clone(),
    )
}

/// The app's SQLite store. If the file can't be opened (read-only home,
/// corrupt file), data lives in memory for this session instead of the app
/// refusing to start.
fn metadata_store(runtime: &Handle) -> Result<Arc<SqliteStore>, StoreError> {
    let path = platform_dir(DATA_HOME_VARIABLE, PLATFORM_DATA_HOME).join(METADATA_FILE);
    match SqliteStore::open(&path, runtime.clone()) {
        Ok(store) => Ok(Arc::new(store)),
        Err(error) => {
            eprintln!("FinderMaxxing: {} unavailable ({error}); settings will not persist", path.display());
            SqliteStore::in_memory(runtime.clone()).map(Arc::new)
        }
    }
}

/// `~/Library/<…>/FinderMaxxing` on macOS; the XDG directory (or its
/// default below home) elsewhere.
fn platform_dir(xdg_variable: &str, platform_default: &str) -> PathBuf {
    let xdg = std::env::var_os(xdg_variable).map(PathBuf::from).filter(|p| p.is_absolute());
    let base = if cfg!(target_os = "macos") { None } else { xdg };
    base.unwrap_or_else(|| home_dir().join(platform_default)).join(APP_FOLDER)
}

fn home_dir() -> PathBuf {
    std::env::var_os(HOME_VARIABLE).map_or_else(|| PathBuf::from(FALLBACK_HOME), PathBuf::from)
}
