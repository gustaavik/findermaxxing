//! Local embedding models on candle, pure Rust (ADR 0026):
//!
//! - [`LocalTextEmbedder`]: multilingual-e5-small (MIT), 384 dimensions,
//!   ~100 languages, for documents and text queries;
//! - [`LocalImageEmbedder`]: CLIP ViT-B/32 (MIT) for images and the
//!   multilingual CLIP text encoder (Apache-2.0) for queries, 512 dimensions;
//! - [`HuggingFaceInstaller`]: downloads them from pinned revisions with
//!   SHA-256 checks, after the user agreed.
//!
//! Models load on first use and stay loaded. Inference runs on its own
//! thread pool with half the cores, so indexing never takes the whole machine.

mod image;
mod installer;
mod manifest;
mod prepare;
mod text;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use application::ports::{EmbedError, Embedding, ImageEmbedder, LocalFile, ModelId, ModelPack, TextEmbedder};
use candle_core::{DType, Tensor};
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::runtime::Handle;

pub use installer::HuggingFaceInstaller;

pub(crate) type EmbedResult<T> = Result<T, EmbedError>;

/// Passages embedded together (bounded padding and memory).
const PASSAGE_BATCH: usize = 8;
/// e5 similarities sit high and close together (measured: matches 0.82–0.86,
/// unrelated documents up to 0.81), so the floor only trims obvious noise;
/// ranking does the rest.
const E5_FLOOR: f32 = 0.79;
/// CLIP text-to-image similarities are low (matches 0.26–0.31, others ≤ 0.23).
const CLIP_FLOOR: f32 = 0.22;
const INSTALLED_MARKER: &str = "installed";

pub(crate) fn failed(error: &dyn std::fmt::Display) -> EmbedError {
    EmbedError::Failed(error.to_string())
}

/// Mean of the token vectors where `mask` is 1: `(batch, tokens, dim)` → `(batch, dim)`.
pub(crate) fn mean_pool(hidden: &Tensor, mask: &Tensor) -> EmbedResult<Tensor> {
    let mask = mask.to_dtype(DType::F32).and_then(|m| m.unsqueeze(2)).map_err(|e| failed(&e))?;
    let summed = hidden.broadcast_mul(&mask).and_then(|t| t.sum(1)).map_err(|e| failed(&e))?;
    let counts = mask.sum(1).and_then(|c| c.clamp(1e-9, f64::MAX)).map_err(|e| failed(&e))?;
    summed.broadcast_div(&counts).map_err(|e| failed(&e))
}

/// Rows scaled to unit length, as vectors.
pub(crate) fn normalise(rows: &Tensor) -> EmbedResult<Vec<Vec<f32>>> {
    let norms = rows.sqr().and_then(|s| s.sum_keepdim(1)).and_then(|s| s.sqrt()).map_err(|e| failed(&e))?;
    let unit = rows.broadcast_div(&norms.clamp(1e-12, f64::MAX).map_err(|e| failed(&e))?).map_err(|e| failed(&e))?;
    unit.to_dtype(DType::F32).and_then(|t| t.to_vec2::<f32>()).map_err(|e| failed(&e))
}

/// Runs model work on the blocking pool, inside the inference thread pool.
#[derive(Clone)]
struct Inference {
    runtime: Handle,
    pool: Arc<rayon::ThreadPool>,
}

impl Inference {
    fn new(runtime: Handle) -> Self {
        let threads = std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1));
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("findermaxxing-inference-{i}"))
            .build()
            .or_else(|_| rayon::ThreadPoolBuilder::new().num_threads(1).build());
        let pool = pool.unwrap_or_else(|_| unreachable!("a one-thread pool can always be built"));
        Self { runtime, pool: Arc::new(pool) }
    }

    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> EmbedResult<T> + Send + 'static,
    ) -> BoxFuture<'static, EmbedResult<T>> {
        let pool = Arc::clone(&self.pool);
        let task = self.runtime.spawn_blocking(move || pool.install(work));
        async move { task.await.map_err(|e| failed(&e))? }.boxed()
    }
}

/// A model loaded on first use, if its pack is installed.
struct Lazy<M> {
    dir: PathBuf,
    loaded: Mutex<Option<Arc<M>>>,
}

impl<M> Lazy<M> {
    fn new(dir: PathBuf) -> Self {
        Self { dir, loaded: Mutex::new(None) }
    }

    fn get(&self, load: impl FnOnce(&Path) -> EmbedResult<M>) -> EmbedResult<Arc<M>> {
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(model) = loaded.as_ref() {
            return Ok(Arc::clone(model));
        }
        if !self.dir.join(INSTALLED_MARKER).exists() {
            return Err(EmbedError::Missing);
        }
        let model = Arc::new(load(&self.dir)?);
        *loaded = Some(Arc::clone(&model));
        Ok(model)
    }
}

/// Folders of the installed packs below the models root.
fn pack_dir(root: &Path, pack: ModelPack) -> PathBuf {
    root.join(manifest::pack_folder(pack))
}

pub struct LocalTextEmbedder {
    model: Arc<Lazy<text::E5>>,
    inference: Inference,
}

impl LocalTextEmbedder {
    #[must_use]
    pub fn new(models_root: &Path, runtime: Handle) -> Self {
        Self { model: Arc::new(Lazy::new(pack_dir(models_root, ModelPack::Text))), inference: Inference::new(runtime) }
    }
}

impl TextEmbedder for LocalTextEmbedder {
    fn model(&self) -> ModelId {
        ModelId(manifest::model_id(ModelPack::Text))
    }

    fn relevance_floor(&self) -> f32 {
        E5_FLOOR
    }

    fn embed_passages(&self, passages: Vec<String>) -> BoxFuture<'static, Result<Vec<Embedding>, EmbedError>> {
        let model = Arc::clone(&self.model);
        self.inference.run(move || {
            let model = model.get(text::E5::load)?;
            let prefixed: Vec<String> = passages.iter().map(|p| format!("{}{p}", text::PASSAGE_PREFIX)).collect();
            let mut vectors = Vec::with_capacity(prefixed.len());
            for batch in prefixed.chunks(PASSAGE_BATCH) {
                vectors.extend(model.embed(batch)?);
            }
            Ok(vectors)
        })
    }

    fn embed_query(&self, query: &str) -> BoxFuture<'static, Result<Embedding, EmbedError>> {
        let (model, query) = (Arc::clone(&self.model), format!("{}{query}", text::QUERY_PREFIX));
        self.inference.run(move || {
            let model = model.get(text::E5::load)?;
            model.embed(&[query])?.into_iter().next().ok_or_else(|| failed(&"no vector"))
        })
    }
}

pub struct LocalImageEmbedder {
    text: Arc<Lazy<image::ClipText>>,
    vision: Arc<Lazy<image::ClipVision>>,
    inference: Inference,
}

impl LocalImageEmbedder {
    #[must_use]
    pub fn new(models_root: &Path, runtime: Handle) -> Self {
        let dir = pack_dir(models_root, ModelPack::Images);
        Self {
            text: Arc::new(Lazy::new(dir.clone())),
            vision: Arc::new(Lazy::new(dir)),
            inference: Inference::new(runtime),
        }
    }
}

impl ImageEmbedder for LocalImageEmbedder {
    fn model(&self) -> ModelId {
        ModelId(manifest::model_id(ModelPack::Images))
    }

    fn relevance_floor(&self) -> f32 {
        CLIP_FLOOR
    }

    fn embed_image(&self, file: LocalFile) -> BoxFuture<'static, Result<Option<Embedding>, EmbedError>> {
        let vision = Arc::clone(&self.vision);
        self.inference.run(move || vision.get(image::ClipVision::load)?.embed(file.path()))
    }

    fn embed_query(&self, query: &str) -> BoxFuture<'static, Result<Embedding, EmbedError>> {
        let (model, query) = (Arc::clone(&self.text), query.to_owned());
        self.inference.run(move || model.get(image::ClipText::load)?.embed(&query))
    }
}

impl std::fmt::Debug for LocalTextEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalTextEmbedder").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for LocalImageEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalImageEmbedder").finish_non_exhaustive()
    }
}
