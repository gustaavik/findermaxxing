//! The content index (ADR 0025): local models, text extraction, the SQLite
//! vector index, and the background indexer, started once at launch.

use std::path::Path;
use std::sync::Arc;

use adapter_candle_embeddings::{HuggingFaceInstaller, LocalImageEmbedder, LocalTextEmbedder};
use adapter_content_extract::LocalExtractor;
use adapter_local_fs::storage_path_from_native;
use adapter_sqlite_store::SqliteStore;
use adapter_tokio_runtime::TokioClock;
use application::ports::{AiSettingsStore, ImageEmbedder, ModelInstaller, MountResolver, SemanticIndex, TextEmbedder};
use application::use_cases::{AiSearchServices, IndexerParts, SemanticSearch, indexer};
use domain::VfsError;
use presentation::{AiSearch, IndexStatusFeed};
use tokio::runtime::Handle;

/// Below the app's data folder: models are large, but not a cache to purge.
const MODELS_FOLDER: &str = "models";
/// Searched when AI search is first turned on; the user changes this list.
const DEFAULT_FOLDERS: [&str; 4] = ["Documents", "Desktop", "Downloads", "Pictures"];

pub(crate) fn start(
    runtime: &Handle,
    mounts: &Arc<dyn MountResolver>,
    store: &Arc<SqliteStore>,
    home: &Path,
) -> Result<AiSearch, VfsError> {
    let models_root = crate::platform_dir(crate::DATA_HOME_VARIABLE, crate::PLATFORM_DATA_HOME).join(MODELS_FOLDER);
    let installer: Arc<dyn ModelInstaller> = Arc::new(HuggingFaceInstaller::new(models_root.clone(), runtime.clone()));
    let text: Arc<dyn TextEmbedder> = Arc::new(LocalTextEmbedder::new(&models_root, runtime.clone()));
    let images: Arc<dyn ImageEmbedder> = Arc::new(LocalImageEmbedder::new(&models_root, runtime.clone()));
    let index: Arc<dyn SemanticIndex> = store.clone();
    let settings: Arc<dyn AiSettingsStore> = store.clone();
    let status = Arc::new(IndexStatusFeed::default());
    let (worker, control) = indexer(IndexerParts {
        mounts: mounts.clone(),
        extractor: Arc::new(LocalExtractor::new(runtime.clone())),
        text: text.clone(),
        images: images.clone(),
        index: index.clone(),
        settings: settings.clone(),
        installer: installer.clone(),
        clock: Arc::new(TokioClock),
        observer: status.clone(),
    });
    runtime.spawn(worker.run());
    let default_scope = DEFAULT_FOLDERS
        .iter()
        .map(|folder| storage_path_from_native(&home.join(folder)))
        .collect::<Result<Vec<_>, _>>()?;
    let services = AiSearchServices {
        settings: settings.clone(),
        installer: installer.clone(),
        index: index.clone(),
        control,
        default_scope,
    };
    let search = SemanticSearch::new(text, images, index, settings, installer);
    Ok(AiSearch::new(&services, search, status))
}
