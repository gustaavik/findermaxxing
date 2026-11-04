//! `AccountStore`: S3 account settings. The stored strings (`"access_key"`,
//! `"path"`) are a storage format, independent of Rust names.

use application::ports::{AccountStore, NewS3Account, StoreError};
use domain::{AccountId, Addressing, BucketName, S3Account, S3Auth};
use futures::future::BoxFuture;
use rusqlite::{Row, params};

use crate::{SqliteStore, unavailable};

const AUTH_DEFAULT: &str = "default";
const AUTH_PROFILE: &str = "profile";
const AUTH_ACCESS_KEY: &str = "access_key";
const ADDRESSING_VIRTUAL: &str = "virtual";
const ADDRESSING_PATH: &str = "path";
const COLUMNS: &str =
    "id, name, endpoint, region, auth_kind, auth_value, addressing, skip_cost_warnings, extra_buckets";
/// `extra_buckets` holds one bucket name per line (names never contain newlines).
const BUCKET_SEPARATOR: char = '\n';

fn auth_columns(auth: &S3Auth) -> (&'static str, Option<String>) {
    match auth {
        S3Auth::DefaultChain => (AUTH_DEFAULT, None),
        S3Auth::Profile(profile) => (AUTH_PROFILE, Some(profile.clone())),
        S3Auth::AccessKey { access_key_id } => (AUTH_ACCESS_KEY, Some(access_key_id.clone())),
    }
}

fn addressing_column(addressing: Addressing) -> &'static str {
    match addressing {
        Addressing::VirtualHosted => ADDRESSING_VIRTUAL,
        Addressing::PathStyle => ADDRESSING_PATH,
    }
}

struct AccountRow {
    id: i64,
    name: String,
    endpoint: Option<String>,
    region: Option<String>,
    auth_kind: String,
    auth_value: Option<String>,
    addressing: String,
    skip_cost_warnings: bool,
    extra_buckets: String,
}

impl AccountRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            endpoint: row.get(2)?,
            region: row.get(3)?,
            auth_kind: row.get(4)?,
            auth_value: row.get(5)?,
            addressing: row.get(6)?,
            skip_cost_warnings: row.get(7)?,
            extra_buckets: row.get(8)?,
        })
    }

    fn decode(self) -> Result<S3Account, StoreError> {
        let corrupt = |what: &str| StoreError::Corrupt(format!("account {}: unknown {what}", self.id));
        let auth = match (self.auth_kind.as_str(), self.auth_value) {
            (AUTH_DEFAULT, _) => S3Auth::DefaultChain,
            (AUTH_PROFILE, Some(profile)) => S3Auth::Profile(profile),
            (AUTH_ACCESS_KEY, Some(access_key_id)) => S3Auth::AccessKey { access_key_id },
            _ => return Err(corrupt("authentication")),
        };
        let addressing = match self.addressing.as_str() {
            ADDRESSING_VIRTUAL => Addressing::VirtualHosted,
            ADDRESSING_PATH => Addressing::PathStyle,
            _ => return Err(corrupt("addressing")),
        };
        let extra_buckets = self
            .extra_buckets
            .split(BUCKET_SEPARATOR)
            .filter(|name| !name.is_empty())
            .map(|name| BucketName::new(name).map_err(|_| corrupt("bucket name")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(S3Account {
            id: AccountId(self.id),
            name: self.name,
            endpoint: self.endpoint,
            region: self.region,
            auth,
            addressing,
            skip_cost_warnings: self.skip_cost_warnings,
            extra_buckets,
        })
    }
}

impl AccountStore for SqliteStore {
    fn list_s3(&self) -> BoxFuture<'static, Result<Vec<S3Account>, StoreError>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare(&format!("SELECT {COLUMNS} FROM s3_accounts ORDER BY name COLLATE NOCASE, id"))
                .map_err(|e| unavailable(&e))?;
            let rows = statement.query_map([], AccountRow::read).map_err(|e| unavailable(&e))?;
            rows.map(|row| row.map_err(|e| unavailable(&e)).and_then(AccountRow::decode)).collect()
        })
    }

    fn add_s3(&self, account: NewS3Account) -> BoxFuture<'static, Result<S3Account, StoreError>> {
        self.run(move |connection| {
            let (auth_kind, auth_value) = auth_columns(&account.auth);
            connection
                .execute(
                    "INSERT INTO s3_accounts (name, endpoint, region, auth_kind, auth_value, addressing)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        account.name,
                        account.endpoint,
                        account.region,
                        auth_kind,
                        auth_value,
                        addressing_column(account.addressing)
                    ],
                )
                .map_err(|e| unavailable(&e))?;
            Ok(S3Account {
                id: AccountId(connection.last_insert_rowid()),
                name: account.name,
                endpoint: account.endpoint,
                region: account.region,
                auth: account.auth,
                addressing: account.addressing,
                skip_cost_warnings: false,
                extra_buckets: Vec::new(),
            })
        })
    }

    fn remove(&self, id: AccountId) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection.execute("DELETE FROM s3_accounts WHERE id = ?1", [id.0]).map_err(|e| unavailable(&e)).map(drop)
        })
    }

    fn set_skip_cost_warnings(&self, id: AccountId, skip: bool) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute("UPDATE s3_accounts SET skip_cost_warnings = ?2 WHERE id = ?1", params![id.0, skip])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn add_bucket(&self, id: AccountId, bucket: BucketName) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            let current: String = connection
                .query_row("SELECT extra_buckets FROM s3_accounts WHERE id = ?1", [id.0], |row| row.get(0))
                .map_err(|e| unavailable(&e))?;
            let mut names: Vec<&str> = current.split(BUCKET_SEPARATOR).filter(|n| !n.is_empty()).collect();
            if !names.contains(&bucket.as_str()) {
                names.push(bucket.as_str());
            }
            let joined = names.join(&BUCKET_SEPARATOR.to_string());
            connection
                .execute("UPDATE s3_accounts SET extra_buckets = ?2 WHERE id = ?1", params![id.0, joined])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }
}
