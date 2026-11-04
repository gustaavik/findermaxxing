use domain::{SavedSearch, SavedSearchId, SearchQuery, StoragePath};
use futures::future::BoxFuture;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("the metadata store is unavailable: {0}")]
    Unavailable(String),
    #[error("the metadata store contains unreadable data: {0}")]
    Corrupt(String),
}

/// A search not yet stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSavedSearch {
    pub name: String,
    pub scope: StoragePath,
    pub query: SearchQuery,
}

/// Persistent saved searches. Object-safe: only one implementation is ever
/// active, so there is no static twin.
pub trait SavedSearchStore: Send + Sync {
    /// All saved searches, ordered by name.
    fn list(&self) -> BoxFuture<'static, Result<Vec<SavedSearch>, StoreError>>;

    fn save(&self, search: NewSavedSearch) -> BoxFuture<'static, Result<SavedSearch, StoreError>>;

    fn delete(&self, id: SavedSearchId) -> BoxFuture<'static, Result<(), StoreError>>;
}
