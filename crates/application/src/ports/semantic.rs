//! Ports of the content index (ADR 0025): embedding models, text extraction,
//! the vector index, settings, and installing the models.

use domain::{ByteSize, ContentKind, DateRange, FileCategory, Fingerprint, IndexScope, StoragePath, VfsError};
use futures::future::BoxFuture;

use super::mutation::ProgressCallback;
use super::preview::LocalFile;
use super::saved_searches::StoreError;
use crate::cancel::CancelToken;

/// An L2-normalised vector; similarity is the dot product.
pub type Embedding = Vec<f32>;

/// Names an embedding space (model and revision). Vectors from different
/// spaces are never compared: changing models means re-indexing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmbedError {
    /// The model isn't installed (yet).
    #[error("the model isn't installed")]
    Missing,
    #[error("the model failed: {0}")]
    Failed(String),
}

/// Text into the shared text space (documents and queries).
pub trait TextEmbedder: Send + Sync {
    fn model(&self) -> ModelId;

    /// Similarities below this are noise for this model.
    fn relevance_floor(&self) -> f32;

    /// Documents whose first passages are at least this similar, on
    /// average, are about the same thing (used to suggest folders).
    fn grouping_threshold(&self) -> f32;

    fn embed_passages(&self, passages: Vec<String>) -> BoxFuture<'static, Result<Vec<Embedding>, EmbedError>>;

    fn embed_query(&self, query: &str) -> BoxFuture<'static, Result<Embedding, EmbedError>>;
}

/// Images and text queries into one shared space (CLIP).
pub trait ImageEmbedder: Send + Sync {
    fn model(&self) -> ModelId;

    fn relevance_floor(&self) -> f32;

    /// `None` for an image that can't be decoded.
    fn embed_image(&self, file: LocalFile) -> BoxFuture<'static, Result<Option<Embedding>, EmbedError>>;

    fn embed_query(&self, query: &str) -> BoxFuture<'static, Result<Embedding, EmbedError>>;
}

/// Plain text of a document (text files, PDF, Office/OpenDocument, HTML).
pub trait ContentExtractor: Send + Sync {
    /// `None` when the file has no extractable text.
    fn extract(&self, file: &LocalFile, category: FileCategory)
    -> BoxFuture<'static, Result<Option<String>, VfsError>>;
}

/// One passage of an indexed document.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedPassage {
    /// The start of the passage, shown as the result's snippet.
    pub snippet: String,
    pub vector: Embedding,
}

/// What the index keeps about an item: enough to show it as a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedItem {
    pub location: StoragePath,
    pub kind: ContentKind,
    pub category: FileCategory,
    pub fingerprint: Fingerprint,
    pub model: ModelId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexHit {
    pub item: IndexedItem,
    /// Best passage similarity; `0.0` for filter-only queries.
    pub similarity: f32,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexQuery {
    /// `None`: no meaning to match, only the filters (newest first).
    pub vector: Option<Embedding>,
    pub kind: ContentKind,
    pub model: ModelId,
    pub limit: usize,
    pub modified: Option<DateRange>,
    pub category: Option<FileCategory>,
    /// Only items within one of these folders; empty means everywhere.
    pub within: Vec<StoragePath>,
}

pub trait SemanticIndex: Send + Sync {
    /// What was indexed for `location`, if anything.
    fn indexed(&self, location: &StoragePath) -> BoxFuture<'static, Result<Option<IndexedItem>, StoreError>>;

    /// Replaces whatever was indexed for the item. No passages means "seen,
    /// nothing to index" (so it isn't read again until it changes).
    fn upsert(&self, item: IndexedItem, passages: Vec<IndexedPassage>) -> BoxFuture<'static, Result<(), StoreError>>;

    fn remove(&self, locations: Vec<StoragePath>) -> BoxFuture<'static, Result<(), StoreError>>;

    /// Every indexed location within `folder`, to find what disappeared.
    fn locations_within(&self, folder: &StoragePath) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>>;

    /// Every indexed location, to forget folders taken off the scope.
    fn all_locations(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>>;

    fn search(&self, query: IndexQuery) -> BoxFuture<'static, Result<Vec<IndexHit>, StoreError>>;

    /// Items with at least one passage.
    fn count(&self) -> BoxFuture<'static, Result<u64, StoreError>>;

    fn clear(&self) -> BoxFuture<'static, Result<(), StoreError>>;
}

/// The user's choices for AI search.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiSettings {
    pub enabled: bool,
    pub images: bool,
    pub paused: bool,
    pub scope: IndexScope,
}

pub trait AiSettingsStore: Send + Sync {
    /// `None` before the user has ever chosen.
    fn load(&self) -> BoxFuture<'static, Result<Option<AiSettings>, StoreError>>;
    fn save(&self, settings: AiSettings) -> BoxFuture<'static, Result<(), StoreError>>;
}

/// The model downloads the user agrees to separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelPack {
    /// Multilingual text search.
    Text,
    /// Searching photos by what they show.
    Images,
}

/// Fetches models from their pinned source, verifying every file.
pub trait ModelInstaller: Send + Sync {
    fn installed(&self, pack: ModelPack) -> bool;

    /// Bytes to download (shown before the user agrees).
    fn download_size(&self, pack: ModelPack) -> ByteSize;

    /// Where the files come from, shown before the user agrees.
    fn source(&self, pack: ModelPack) -> String;

    fn install(
        &self,
        pack: ModelPack,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> BoxFuture<'static, Result<(), VfsError>>;

    fn uninstall(&self, pack: ModelPack) -> BoxFuture<'static, Result<(), VfsError>>;
}

/// Where the indexer reports what it is doing.
pub trait IndexObserver: Send + Sync {
    fn status(&self, status: &domain::IndexStatus);
}
