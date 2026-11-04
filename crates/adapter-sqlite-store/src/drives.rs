//! `DriveAccountStore`: cloud-drive accounts. Refresh tokens live in the keychain.

use application::ports::{DriveAccountStore, NewDriveAccount, StoreError};
use domain::{AccountId, DriveAccount, DriveProvider};
use futures::future::BoxFuture;
use rusqlite::params;

use crate::location_codec::corrupt;
use crate::{SqliteStore, unavailable};

const GOOGLE_DRIVE: &str = "google_drive";
const DROPBOX: &str = "dropbox";
const ONEDRIVE: &str = "onedrive";

fn provider_column(provider: DriveProvider) -> &'static str {
    match provider {
        DriveProvider::GoogleDrive => GOOGLE_DRIVE,
        DriveProvider::Dropbox => DROPBOX,
        DriveProvider::OneDrive => ONEDRIVE,
    }
}

fn provider_of(column: &str) -> Result<DriveProvider, StoreError> {
    match column {
        GOOGLE_DRIVE => Ok(DriveProvider::GoogleDrive),
        DROPBOX => Ok(DriveProvider::Dropbox),
        ONEDRIVE => Ok(DriveProvider::OneDrive),
        other => Err(corrupt(format!("unknown drive provider {other:?}"))),
    }
}

impl DriveAccountStore for SqliteStore {
    fn list_drives(&self) -> BoxFuture<'static, Result<Vec<DriveAccount>, StoreError>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare("SELECT id, provider, name, email FROM drive_accounts ORDER BY name COLLATE NOCASE, id")
                .map_err(|e| unavailable(&e))?;
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get(2)?, row.get(3)?)))
                .map_err(|e| unavailable(&e))?;
            rows.map(|row| {
                let (id, provider, name, email) = row.map_err(|e| unavailable(&e))?;
                Ok(DriveAccount { id: AccountId(id), provider: provider_of(&provider)?, name, email })
            })
            .collect()
        })
    }

    fn add_drive(&self, account: NewDriveAccount) -> BoxFuture<'static, Result<DriveAccount, StoreError>> {
        self.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO drive_accounts (provider, name, email) VALUES (?1, ?2, ?3)",
                    params![provider_column(account.provider), account.name, account.email],
                )
                .map_err(|e| unavailable(&e))?;
            Ok(account.with_id(AccountId(connection.last_insert_rowid())))
        })
    }

    fn remove_drive(&self, id: AccountId) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute("DELETE FROM drive_accounts WHERE id = ?1", [id.0])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }
}
