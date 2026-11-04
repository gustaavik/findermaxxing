//! Composition root. Builds the runtime, registers providers, constructs use
//! cases with their ports, and hands the result to the presentation layer.
//! No other crate names concrete adapters.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use adapter_local_fs::{LocalFs, LocalFsFactory, LocalTrash, storage_path_from_native};
use adapter_preview::{LocalArchiveInspector, SyntectTextRenderer};
use adapter_sqlite_store::SqliteStore;
use adapter_tokio_runtime::TokioClock;
use application::ProviderRegistry;
use application::ports::{MountResolver, NewSavedSearch, SavedSearchStore, StoreError};
use application::use_cases::{CoalesceWindow, ListDirectory, PreparePreview, SearchFiles, WatchDirectory};
use domain::{SavedSearch, SavedSearchId};
use futures::FutureExt;
use futures::future::{self, BoxFuture};
use presentation::{AppServices, FileOperations, SavedSearches};
use tokio::runtime::Handle;

const HOME_VARIABLE: &str = "HOME";
const FALLBACK_HOME: &str = "/";
const RUNTIME_THREAD_NAME: &str = "findermaxxing-worker";
const APP_DATA_FOLDER: &str = "FinderMaxxing";
const METADATA_FILE: &str = "metadata.sqlite3";
#[cfg(target_os = "macos")]
const PLATFORM_DATA_HOME: &str = "Library/Application Support";
#[cfg(not(target_os = "macos"))]
const PLATFORM_DATA_HOME: &str = ".local/share";
const DATA_HOME_VARIABLE: &str = "XDG_DATA_HOME";

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
    let registry = build_registry(runtime.handle().clone())?;
    let handle = runtime.handle().clone();
    let services = AppServices {
        list_directory: Arc::new(ListDirectory::new(registry.clone())),
        watch_directory: Arc::new(WatchDirectory::new(
            registry.clone(),
            Arc::new(TokioClock),
            CoalesceWindow::default(),
        )),
        search: Arc::new(SearchFiles::new(registry.clone())),
        preview: Arc::new(PreparePreview::new(
            registry.clone(),
            Arc::new(SyntectTextRenderer::new(handle.clone())),
            Arc::new(LocalArchiveInspector::new(handle.clone())),
        )),
        saved_searches: SavedSearches::new(&metadata_store(&handle)),
        operations: FileOperations::new(&registry),
        runtime: handle,
        home: storage_path_from_native(&home_dir())?,
    };
    let exit_code = presentation::run(services);
    // Background tasks are cancelled with their sessions; don't wait on stragglers.
    runtime.shutdown_background();
    Ok(exit_code)
}

/// Every provider is registered here and nowhere else (Open/Closed).
fn build_registry(runtime: Handle) -> Result<Arc<dyn MountResolver>, StartupError> {
    let local = LocalFsFactory::new(LocalFs::new(runtime.clone()), LocalTrash::platform_default(runtime))?;
    Ok(Arc::new(ProviderRegistry::default().with_provider(Arc::new(local))?))
}

/// The app's SQLite store. If it can't be opened (read-only home, corrupt
/// file), saved searches live in memory for this session instead of the app
/// refusing to start.
fn metadata_store(runtime: &Handle) -> Arc<dyn SavedSearchStore> {
    let path = data_dir().join(METADATA_FILE);
    match SqliteStore::open(&path, runtime.clone()) {
        Ok(store) => Arc::new(store),
        Err(error) => {
            eprintln!("FinderMaxxing: {} unavailable ({error}); saved searches will not persist", path.display());
            match SqliteStore::in_memory(runtime.clone()) {
                Ok(store) => Arc::new(store),
                Err(_) => Arc::new(NoSavedSearches),
            }
        }
    }
}

/// `~/Library/Application Support/FinderMaxxing` on macOS,
/// `$XDG_DATA_HOME/FinderMaxxing` (default `~/.local/share/…`) elsewhere.
fn data_dir() -> PathBuf {
    let xdg = std::env::var_os(DATA_HOME_VARIABLE).map(PathBuf::from).filter(|p| p.is_absolute());
    let base = if cfg!(target_os = "macos") { None } else { xdg };
    base.unwrap_or_else(|| home_dir().join(PLATFORM_DATA_HOME)).join(APP_DATA_FOLDER)
}

/// Last resort when even an in-memory SQLite fails: an always-empty store.
#[derive(Debug)]
struct NoSavedSearches;

impl SavedSearchStore for NoSavedSearches {
    fn list(&self) -> BoxFuture<'static, Result<Vec<SavedSearch>, StoreError>> {
        future::ready(Ok(Vec::new())).boxed()
    }

    fn save(&self, _search: NewSavedSearch) -> BoxFuture<'static, Result<SavedSearch, StoreError>> {
        future::ready(Err(StoreError::Unavailable("no metadata store".to_owned()))).boxed()
    }

    fn delete(&self, _id: SavedSearchId) -> BoxFuture<'static, Result<(), StoreError>> {
        future::ready(Ok(())).boxed()
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os(HOME_VARIABLE).map_or_else(|| PathBuf::from(FALLBACK_HOME), PathBuf::from)
}
