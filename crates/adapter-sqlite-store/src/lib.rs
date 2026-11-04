//! SQLite-backed metadata store. Implements `SavedSearchStore`,
//! `AccountStore` and `ServerAccountStore` (settings only: secrets live in
//! the OS keychain), `FavoritesStore`, `PinStore`, `CacheIndex` and
//! `TransferJournal`.
//!
//! The schema is versioned with `PRAGMA user_version`; each entry of
//! [`MIGRATIONS`] runs exactly once, in order, inside a transaction.

mod accounts;
mod cache;
mod favorites;
mod journal;
mod location_codec;
mod query_format;
mod servers;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use application::ports::{NewSavedSearch, SavedSearchStore, StoreError};
use domain::{SavedSearch, SavedSearchId, StoragePath};
use futures::FutureExt;
use futures::future::BoxFuture;
use rusqlite::{Connection, params};
use tokio::runtime::Handle;

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE saved_searches (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        scope TEXT NOT NULL,
        query TEXT NOT NULL
    );",
    "CREATE TABLE s3_accounts (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        endpoint TEXT,
        region TEXT,
        auth_kind TEXT NOT NULL,
        auth_value TEXT,
        addressing TEXT NOT NULL,
        skip_cost_warnings INTEGER NOT NULL DEFAULT 0
    );",
    "CREATE TABLE favorites (
        location TEXT PRIMARY KEY,
        position INTEGER NOT NULL
    );",
    "ALTER TABLE s3_accounts ADD COLUMN extra_buckets TEXT NOT NULL DEFAULT '';",
    // Account ids name keychain items, so they must never be reused: a new
    // account must not inherit a stale secret left by a removed one.
    "CREATE TABLE s3_accounts_v2 (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        endpoint TEXT,
        region TEXT,
        auth_kind TEXT NOT NULL,
        auth_value TEXT,
        addressing TEXT NOT NULL,
        skip_cost_warnings INTEGER NOT NULL DEFAULT 0,
        extra_buckets TEXT NOT NULL DEFAULT ''
    );
    INSERT INTO s3_accounts_v2 SELECT id, name, endpoint, region, auth_kind, auth_value, addressing,
        skip_cost_warnings, extra_buckets FROM s3_accounts;
    DROP TABLE s3_accounts;
    ALTER TABLE s3_accounts_v2 RENAME TO s3_accounts;",
    "CREATE TABLE server_accounts (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        protocol TEXT NOT NULL,
        name TEXT NOT NULL,
        host TEXT NOT NULL,
        port INTEGER,
        transport TEXT NOT NULL,
        username TEXT NOT NULL,
        root TEXT NOT NULL,
        auth_kind TEXT NOT NULL,
        auth_value TEXT
    );",
    "CREATE TABLE pins (location TEXT PRIMARY KEY);
    CREATE TABLE cache_entries (
        location TEXT PRIMARY KEY,
        file TEXT NOT NULL,
        size INTEGER NOT NULL,
        remote_modified INTEGER,
        last_used INTEGER NOT NULL
    );
    CREATE TABLE cache_listings (folder TEXT PRIMARY KEY, entries TEXT NOT NULL);",
    "CREATE TABLE transfer_journal (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL,
        sources TEXT NOT NULL,
        destination TEXT NOT NULL,
        in_flight TEXT
    );",
];

/// One connection, serialised by a mutex: metadata writes are rare and tiny.
#[derive(Clone)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
    runtime: Handle,
}

impl SqliteStore {
    /// Opens (creating if needed) the database file and its folder.
    ///
    /// # Errors
    /// `Unavailable` if the file cannot be opened or migrated.
    pub fn open(path: &Path, runtime: Handle) -> Result<Self, StoreError> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| unavailable(&e))?;
        }
        let connection = Connection::open(path).map_err(|e| unavailable(&e))?;
        connection.pragma_update(None, "journal_mode", "WAL").map_err(|e| unavailable(&e))?;
        Self::with_connection(connection, runtime)
    }

    /// # Errors
    /// `Unavailable` if migration fails.
    pub fn in_memory(runtime: Handle) -> Result<Self, StoreError> {
        Self::with_connection(Connection::open_in_memory().map_err(|e| unavailable(&e))?, runtime)
    }

    fn with_connection(mut connection: Connection, runtime: Handle) -> Result<Self, StoreError> {
        migrate(&mut connection)?;
        Ok(Self { connection: Arc::new(Mutex::new(connection)), runtime })
    }

    pub(crate) fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    ) -> BoxFuture<'static, Result<T, StoreError>> {
        let connection = Arc::clone(&self.connection);
        let task = self.runtime.spawn_blocking(move || {
            let connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
            work(&connection)
        });
        async move { task.await.map_err(|e| unavailable(&e))? }.boxed()
    }
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore").finish_non_exhaustive()
    }
}

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let applied: usize = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|e| unavailable(&e))?
        .try_into()
        .unwrap_or_default();
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        let version = i64::try_from(index + 1).map_err(|e| unavailable(&e))?;
        let transaction = connection.transaction().map_err(|e| unavailable(&e))?;
        transaction.execute_batch(migration).map_err(|e| unavailable(&e))?;
        transaction.pragma_update(None, "user_version", version).map_err(|e| unavailable(&e))?;
        transaction.commit().map_err(|e| unavailable(&e))?;
    }
    Ok(())
}

impl SavedSearchStore for SqliteStore {
    fn list(&self) -> BoxFuture<'static, Result<Vec<SavedSearch>, StoreError>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare("SELECT id, name, scope, query FROM saved_searches ORDER BY name COLLATE NOCASE, id")
                .map_err(|e| unavailable(&e))?;
            let rows = statement
                .query_map([], |row| {
                    Ok(StoredRow { id: row.get(0)?, name: row.get(1)?, scope: row.get(2)?, query: row.get(3)? })
                })
                .map_err(|e| unavailable(&e))?;
            rows.map(|row| row.map_err(|e| unavailable(&e)).and_then(StoredRow::decode)).collect()
        })
    }

    fn save(&self, search: NewSavedSearch) -> BoxFuture<'static, Result<SavedSearch, StoreError>> {
        self.run(move |connection| {
            let query = query_format::encode(&search.query)?;
            connection
                .execute(
                    "INSERT INTO saved_searches (name, scope, query) VALUES (?1, ?2, ?3)",
                    params![search.name, search.scope.encode(), query],
                )
                .map_err(|e| unavailable(&e))?;
            let id = SavedSearchId(connection.last_insert_rowid());
            Ok(SavedSearch { id, name: search.name, scope: search.scope, query: search.query })
        })
    }

    fn delete(&self, id: SavedSearchId) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection.execute("DELETE FROM saved_searches WHERE id = ?1", [id.0]).map_err(|e| unavailable(&e))?;
            Ok(())
        })
    }
}

struct StoredRow {
    id: i64,
    name: String,
    scope: String,
    query: String,
}

impl StoredRow {
    fn decode(self) -> Result<SavedSearch, StoreError> {
        let scope = StoragePath::decode(&self.scope).map_err(|e| StoreError::Corrupt(e.to_string()))?;
        Ok(SavedSearch {
            id: SavedSearchId(self.id),
            name: self.name,
            scope,
            query: query_format::decode(&self.query)?,
        })
    }
}

pub(crate) fn unavailable(error: &dyn std::fmt::Display) -> StoreError {
    StoreError::Unavailable(error.to_string())
}

/// Used by tests to check that unknown rows fail loudly rather than silently.
#[doc(hidden)]
pub fn raw_execute(store: &SqliteStore, sql: &str) -> Result<(), StoreError> {
    let connection = store.connection.lock().unwrap_or_else(PoisonError::into_inner);
    connection.execute_batch(sql).map_err(|e| unavailable(&e))
}
