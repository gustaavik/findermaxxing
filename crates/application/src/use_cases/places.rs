use std::sync::Arc;

use domain::StoragePath;

use crate::ports::{FavoritesStore, StoreError, Volume, VolumeLister};

pub struct ListFavorites {
    store: Arc<dyn FavoritesStore>,
}

impl ListFavorites {
    #[must_use]
    pub fn new(store: Arc<dyn FavoritesStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self) -> Result<Vec<StoragePath>, StoreError> {
        self.store.list().await
    }
}

pub struct AddFavorite {
    store: Arc<dyn FavoritesStore>,
}

impl AddFavorite {
    #[must_use]
    pub fn new(store: Arc<dyn FavoritesStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, location: StoragePath) -> Result<(), StoreError> {
        self.store.add(location).await
    }
}

pub struct RemoveFavorite {
    store: Arc<dyn FavoritesStore>,
}

impl RemoveFavorite {
    #[must_use]
    pub fn new(store: Arc<dyn FavoritesStore>) -> Self {
        Self { store }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, location: StoragePath) -> Result<(), StoreError> {
        self.store.remove(location).await
    }
}

pub struct ListVolumes {
    volumes: Arc<dyn VolumeLister>,
}

impl ListVolumes {
    #[must_use]
    pub fn new(volumes: Arc<dyn VolumeLister>) -> Self {
        Self { volumes }
    }

    pub async fn execute(&self) -> Vec<Volume> {
        self.volumes.volumes().await
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

opaque_debug!(ListFavorites, AddFavorite, RemoveFavorite, ListVolumes);
