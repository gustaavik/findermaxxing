use std::sync::Arc;

use domain::{SavedSearch, SavedSearchId};

use crate::ports::{NewSavedSearch, SavedSearchStore, StoreError};

/// Trimmed name a saved search gets when the user leaves it empty.
pub const UNTITLED_SEARCH: &str = "Untitled Search";

pub struct SaveSearch {
    store: Arc<dyn SavedSearchStore>,
}

impl SaveSearch {
    #[must_use]
    pub fn new(store: Arc<dyn SavedSearchStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, mut search: NewSavedSearch) -> Result<SavedSearch, StoreError> {
        search.name = search.name.trim().to_owned();
        if search.name.is_empty() {
            UNTITLED_SEARCH.clone_into(&mut search.name);
        }
        self.store.save(search).await
    }
}

pub struct ListSavedSearches {
    store: Arc<dyn SavedSearchStore>,
}

impl ListSavedSearches {
    #[must_use]
    pub fn new(store: Arc<dyn SavedSearchStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self) -> Result<Vec<SavedSearch>, StoreError> {
        self.store.list().await
    }
}

pub struct DeleteSavedSearch {
    store: Arc<dyn SavedSearchStore>,
}

impl DeleteSavedSearch {
    #[must_use]
    pub fn new(store: Arc<dyn SavedSearchStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, id: SavedSearchId) -> Result<(), StoreError> {
        self.store.delete(id).await
    }
}

macro_rules! opaque_debug {
    ($($type:ident),+) => {
        $(impl std::fmt::Debug for $type {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($type)).finish_non_exhaustive()
            }
        })+
    };
}

opaque_debug!(SaveSearch, ListSavedSearches, DeleteSavedSearch);
