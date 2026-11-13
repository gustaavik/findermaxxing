//! `LicenseStore`: this machine's signed machine file (not a secret: it
//! only works on this machine; the key itself is in the keychain).

use application::ports::{LicenseStore, MachineFile, StoreError};
use futures::future::BoxFuture;
use rusqlite::OptionalExtension;

use crate::{SqliteStore, unavailable};

impl LicenseStore for SqliteStore {
    fn load(&self) -> BoxFuture<'static, Result<Option<MachineFile>, StoreError>> {
        self.run(|connection| {
            connection
                .query_row("SELECT machine_file FROM license WHERE id = 1", [], |row| row.get(0))
                .optional()
                .map(|file: Option<String>| file.map(MachineFile))
                .map_err(|e| unavailable(&e))
        })
    }

    fn save(&self, file: MachineFile) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute("INSERT OR REPLACE INTO license (id, machine_file) VALUES (1, ?1)", [file.0])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn clear(&self) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(|connection| connection.execute("DELETE FROM license", []).map_err(|e| unavailable(&e)).map(drop))
    }
}
