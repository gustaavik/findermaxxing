//! Searching by meaning (ADR 0025): the query is parsed for dates and kinds
//! (`domain::parse_query`), the rest is embedded and matched against the
//! index. Text and image matches come from different models with
//! incomparable scores, so the two lists are merged by rank (reciprocal
//! rank fusion) rather than by score.

use std::collections::HashMap;
use std::sync::Arc;

use domain::{ContentKind, FileCategory, FileEntry, ParsedQuery, QueryClock, StoragePath, parse_query, relevance};

use crate::ports::{
    AiSettingsStore, EmbedError, ImageEmbedder, IndexHit, IndexQuery, ModelInstaller, ModelPack, SemanticIndex,
    StoreError, TextEmbedder,
};
use crate::use_cases::indexer::result_entry;

/// Results per kind before merging.
const CANDIDATES: usize = 200;
pub const MAX_RESULTS: usize = 100;
/// Reciprocal rank fusion constant: higher flattens the merge.
const FUSION_K: f32 = 20.0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SemanticSearchError {
    #[error("AI search is off")]
    Off,
    #[error(transparent)]
    Model(#[from] EmbedError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticResult {
    pub entry: FileEntry,
    pub kind: ContentKind,
    /// The passage that matched best (documents only).
    pub snippet: Option<String>,
    /// For ordering only; not comparable between searches.
    pub score: f32,
}

pub struct SemanticSearch {
    text: Arc<dyn TextEmbedder>,
    images: Arc<dyn ImageEmbedder>,
    index: Arc<dyn SemanticIndex>,
    settings: Arc<dyn AiSettingsStore>,
    installer: Arc<dyn ModelInstaller>,
}

impl SemanticSearch {
    #[must_use]
    pub fn new(
        text: Arc<dyn TextEmbedder>,
        images: Arc<dyn ImageEmbedder>,
        index: Arc<dyn SemanticIndex>,
        settings: Arc<dyn AiSettingsStore>,
        installer: Arc<dyn ModelInstaller>,
    ) -> Self {
        Self { text, images, index, settings, installer }
    }

    /// Best results first. `within` limits results to these folders (empty: everywhere).
    ///
    /// # Errors
    /// `Off` when AI search isn't set up; model and store failures.
    pub async fn execute(
        &self,
        query: &str,
        clock: QueryClock,
        within: Vec<StoragePath>,
    ) -> Result<Vec<SemanticResult>, SemanticSearchError> {
        let settings = self.settings.load().await?.unwrap_or_default();
        if !settings.enabled || !self.installer.installed(ModelPack::Text) {
            return Err(SemanticSearchError::Off);
        }
        let parsed = parse_query(query, clock);
        let images = settings.images && self.installer.installed(ModelPack::Images);
        let mut lists = Vec::new();
        if parsed.category != Some(FileCategory::Image) {
            lists.push(self.search_kind(&parsed, ContentKind::Text, &within).await?);
        }
        if images && matches!(parsed.category, None | Some(FileCategory::Image)) {
            lists.push(self.search_kind(&parsed, ContentKind::Image, &within).await?);
        }
        Ok(merge(lists))
    }

    async fn search_kind(
        &self,
        parsed: &ParsedQuery,
        kind: ContentKind,
        within: &[StoragePath],
    ) -> Result<Vec<SemanticResult>, SemanticSearchError> {
        let (model, floor, vector) = match (kind, parsed.text.trim().is_empty()) {
            (ContentKind::Text, true) => (self.text.model(), 0.0, None),
            (ContentKind::Image, true) => (self.images.model(), 0.0, None),
            (ContentKind::Text, false) => {
                (self.text.model(), self.text.relevance_floor(), Some(self.text.embed_query(&parsed.text).await?))
            }
            (ContentKind::Image, false) => {
                (self.images.model(), self.images.relevance_floor(), Some(self.images.embed_query(&parsed.text).await?))
            }
        };
        let filtered = vector.is_some();
        let query = IndexQuery {
            vector,
            kind,
            model,
            limit: CANDIDATES,
            modified: parsed.modified,
            // Images are their own kind; the category filter is for documents.
            category: parsed.category.filter(|c| *c != FileCategory::Image),
            within: within.to_vec(),
        };
        let hits = self.index.search(query).await?;
        let mut results: Vec<SemanticResult> = hits
            .into_iter()
            .filter(|hit| !filtered || hit.similarity >= floor)
            .filter_map(|hit| to_result(hit, kind, &parsed.text))
            .collect();
        results.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(results)
    }
}

fn to_result(hit: IndexHit, kind: ContentKind, query: &str) -> Option<SemanticResult> {
    let entry = result_entry(&hit.item)?;
    let score = relevance(hit.similarity, &entry.name().display_name(), query);
    Some(SemanticResult { entry, kind, snippet: hit.snippet.filter(|s| !s.is_empty()), score })
}

/// Reciprocal rank fusion of ranked lists; an item in several lists keeps
/// the sum of its contributions.
fn merge(lists: Vec<Vec<SemanticResult>>) -> Vec<SemanticResult> {
    if lists.len() == 1 {
        return lists.into_iter().flatten().take(MAX_RESULTS).collect();
    }
    let mut fused: HashMap<StoragePath, (f32, SemanticResult)> = HashMap::new();
    for list in lists {
        for (rank, result) in (0_u16..).zip(list) {
            let contribution = 1.0 / (FUSION_K + f32::from(rank));
            fused
                .entry(result.entry.path().clone())
                .and_modify(|(score, _)| *score += contribution)
                .or_insert((contribution, result));
        }
    }
    let mut merged: Vec<SemanticResult> =
        fused.into_values().map(|(score, result)| SemanticResult { score, ..result }).collect();
    merged.sort_by(|a, b| b.score.total_cmp(&a.score));
    merged.truncate(MAX_RESULTS);
    merged
}

impl std::fmt::Debug for SemanticSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticSearch").finish_non_exhaustive()
    }
}
