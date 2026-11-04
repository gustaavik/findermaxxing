//! `FavoritesStore`: sidebar favourites in the user's order, stored with the
//! lossless location encoding so non-UTF-8 paths survive.

use application::ports::{FavoritesStore, StoreError};
use domain::StoragePath;
use futures::future::BoxFuture;
use rusqlite::params;

use crate::{SqliteStore, unavailable};

impl FavoritesStore for SqliteStore {
    fn list(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>> {
        self.run(|connection| {
            let mut statement =
                connection.prepare("SELECT location FROM favorites ORDER BY position").map_err(|e| unavailable(&e))?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|e| unavailable(&e))?;
            rows.map(|row| {
                let encoded = row.map_err(|e| unavailable(&e))?;
                StoragePath::decode(&encoded).map_err(|e| StoreError::Corrupt(e.to_string()))
            })
            .collect()
        })
    }

    fn add(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO favorites (location, position)
                     VALUES (?1, (SELECT COALESCE(MAX(position), 0) + 1 FROM favorites))",
                    params![location.encode()],
                )
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn remove(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute("DELETE FROM favorites WHERE location = ?1", params![location.encode()])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }
}
