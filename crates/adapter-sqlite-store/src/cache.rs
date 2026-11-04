//! `CacheIndex` and `PinStore`: bookkeeping for the local cache of remote
//! files. The copies themselves live in the cache folder.

use application::ports::{CacheIndex, CacheRecord, PinStore, StoreError};
use domain::{ByteSize, FileEntry, StoragePath, Timestamp};
use futures::future::BoxFuture;
use rusqlite::{OptionalExtension, Row, params};

use crate::location_codec::{corrupt, decode_entries, decode_location, encode_entries};
use crate::{SqliteStore, unavailable};

fn read_record(row: &Row<'_>) -> rusqlite::Result<(String, String, i64, Option<i64>, i64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
}

fn decode_record(
    (location, file, size, remote_modified, last_used): (String, String, i64, Option<i64>, i64),
) -> Result<CacheRecord, StoreError> {
    Ok(CacheRecord {
        location: decode_location(&location)?,
        file,
        size: ByteSize(u64::try_from(size).map_err(corrupt)?),
        remote_modified: remote_modified.map(Timestamp::from_unix_millis),
        last_used: Timestamp::from_unix_millis(last_used),
    })
}

const RECORD_COLUMNS: &str = "location, file, size, remote_modified, last_used";

impl CacheIndex for SqliteStore {
    fn lookup(&self, location: &StoragePath) -> BoxFuture<'static, Result<Option<CacheRecord>, StoreError>> {
        let encoded = location.encode();
        self.run(move |connection| {
            connection
                .query_row(
                    &format!("SELECT {RECORD_COLUMNS} FROM cache_entries WHERE location = ?1"),
                    [encoded],
                    read_record,
                )
                .optional()
                .map_err(|e| unavailable(&e))?
                .map(decode_record)
                .transpose()
        })
    }

    fn record(&self, record: CacheRecord) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            let size = i64::try_from(record.size.0).map_err(corrupt)?;
            connection
                .execute(
                    &format!("INSERT OR REPLACE INTO cache_entries ({RECORD_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5)"),
                    params![
                        record.location.encode(),
                        record.file,
                        size,
                        record.remote_modified.map(Timestamp::unix_millis),
                        record.last_used.unix_millis()
                    ],
                )
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn touch(&self, location: &StoragePath, at: Timestamp) -> BoxFuture<'static, Result<(), StoreError>> {
        let encoded = location.encode();
        self.run(move |connection| {
            connection
                .execute(
                    "UPDATE cache_entries SET last_used = ?2 WHERE location = ?1",
                    params![encoded, at.unix_millis()],
                )
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn remove(&self, locations: Vec<StoragePath>) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            let mut statement =
                connection.prepare("DELETE FROM cache_entries WHERE location = ?1").map_err(|e| unavailable(&e))?;
            for location in locations {
                statement.execute([location.encode()]).map_err(|e| unavailable(&e))?;
            }
            Ok(())
        })
    }

    fn all(&self) -> BoxFuture<'static, Result<Vec<CacheRecord>, StoreError>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare(&format!("SELECT {RECORD_COLUMNS} FROM cache_entries"))
                .map_err(|e| unavailable(&e))?;
            let rows = statement.query_map([], read_record).map_err(|e| unavailable(&e))?;
            rows.map(|row| row.map_err(|e| unavailable(&e)).and_then(decode_record)).collect()
        })
    }

    fn save_listing(
        &self,
        folder: &StoragePath,
        entries: Vec<FileEntry>,
    ) -> BoxFuture<'static, Result<(), StoreError>> {
        let encoded = folder.encode();
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT OR REPLACE INTO cache_listings (folder, entries) VALUES (?1, ?2)",
                    params![encoded, encode_entries(&entries)?],
                )
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn listed_folders(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>> {
        self.run(Self::listed_folders_now)
    }

    fn remove_listings(&self, folders: Vec<StoragePath>) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            let mut statement =
                connection.prepare("DELETE FROM cache_listings WHERE folder = ?1").map_err(|e| unavailable(&e))?;
            for folder in folders {
                statement.execute([folder.encode()]).map_err(|e| unavailable(&e))?;
            }
            Ok(())
        })
    }

    fn listing(&self, folder: &StoragePath) -> BoxFuture<'static, Result<Option<Vec<FileEntry>>, StoreError>> {
        let encoded = folder.encode();
        self.run(move |connection| {
            connection
                .query_row("SELECT entries FROM cache_listings WHERE folder = ?1", [encoded], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
                .map_err(|e| unavailable(&e))?
                .map(|json| decode_entries(&json))
                .transpose()
        })
    }
}

impl SqliteStore {
    fn listed_folders_now(connection: &rusqlite::Connection) -> Result<Vec<StoragePath>, StoreError> {
        let mut statement = connection.prepare("SELECT folder FROM cache_listings").map_err(|e| unavailable(&e))?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|e| unavailable(&e))?;
        rows.map(|row| decode_location(&row.map_err(|e| unavailable(&e))?)).collect()
    }
}

impl PinStore for SqliteStore {
    fn pins(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>> {
        self.run(|connection| {
            let mut statement = connection.prepare("SELECT location FROM pins").map_err(|e| unavailable(&e))?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|e| unavailable(&e))?;
            rows.map(|row| decode_location(&row.map_err(|e| unavailable(&e))?)).collect()
        })
    }

    fn pin(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute("INSERT OR IGNORE INTO pins (location) VALUES (?1)", [location.encode()])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn unpin(&self, location: StoragePath) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute("DELETE FROM pins WHERE location = ?1", [location.encode()])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }
}
