//! `ServerAccountStore`: file-server settings. Passwords live in the keychain.

use application::ports::{NewServerAccount, ServerAccountStore, StoreError};
use domain::{AccountId, ServerAccount, ServerAuth, ServerProtocol, Transport};
use futures::future::BoxFuture;
use rusqlite::{Row, params};

use crate::location_codec::corrupt;
use crate::{SqliteStore, unavailable};

const COLUMNS: &str = "id, protocol, name, host, port, transport, username, root, auth_kind, auth_value";

const SFTP: &str = "sftp";
const WEBDAV: &str = "webdav";
const SMB: &str = "smb";
const TLS: &str = "tls";
const PLAIN: &str = "plain";
const PASSWORD: &str = "password";
const AGENT: &str = "agent";
const KEY_FILE: &str = "key_file";
const ANONYMOUS: &str = "anonymous";

fn protocol_column(protocol: ServerProtocol) -> &'static str {
    match protocol {
        ServerProtocol::Sftp => SFTP,
        ServerProtocol::WebDav => WEBDAV,
        ServerProtocol::Smb => SMB,
    }
}

fn transport_column(transport: Transport) -> &'static str {
    match transport {
        Transport::Tls => TLS,
        Transport::Plain => PLAIN,
    }
}

fn auth_columns(auth: &ServerAuth) -> (&'static str, Option<&str>) {
    match auth {
        ServerAuth::Password => (PASSWORD, None),
        ServerAuth::Agent => (AGENT, None),
        ServerAuth::KeyFile { path } => (KEY_FILE, Some(path)),
        ServerAuth::Anonymous => (ANONYMOUS, None),
    }
}

struct ServerRow {
    id: i64,
    protocol: String,
    name: String,
    host: String,
    port: Option<i64>,
    transport: String,
    username: String,
    root: String,
    auth_kind: String,
    auth_value: Option<String>,
}

impl ServerRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            protocol: row.get(1)?,
            name: row.get(2)?,
            host: row.get(3)?,
            port: row.get(4)?,
            transport: row.get(5)?,
            username: row.get(6)?,
            root: row.get(7)?,
            auth_kind: row.get(8)?,
            auth_value: row.get(9)?,
        })
    }

    fn decode(self) -> Result<ServerAccount, StoreError> {
        let protocol = match self.protocol.as_str() {
            SFTP => ServerProtocol::Sftp,
            WEBDAV => ServerProtocol::WebDav,
            SMB => ServerProtocol::Smb,
            other => return Err(corrupt(format!("unknown protocol {other:?}"))),
        };
        let transport = match self.transport.as_str() {
            TLS => Transport::Tls,
            PLAIN => Transport::Plain,
            other => return Err(corrupt(format!("unknown transport {other:?}"))),
        };
        let auth = match (self.auth_kind.as_str(), self.auth_value) {
            (PASSWORD, _) => ServerAuth::Password,
            (AGENT, _) => ServerAuth::Agent,
            (KEY_FILE, Some(path)) => ServerAuth::KeyFile { path },
            (ANONYMOUS, _) => ServerAuth::Anonymous,
            (other, _) => return Err(corrupt(format!("unknown sign-in {other:?}"))),
        };
        let port = self.port.map(u16::try_from).transpose().map_err(corrupt)?;
        Ok(ServerAccount {
            id: AccountId(self.id),
            protocol,
            name: self.name,
            host: self.host,
            port,
            transport,
            username: self.username,
            root: self.root,
            auth,
        })
    }
}

impl ServerAccountStore for SqliteStore {
    fn list_servers(&self) -> BoxFuture<'static, Result<Vec<ServerAccount>, StoreError>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare(&format!("SELECT {COLUMNS} FROM server_accounts ORDER BY name COLLATE NOCASE, id"))
                .map_err(|e| unavailable(&e))?;
            let rows = statement.query_map([], ServerRow::read).map_err(|e| unavailable(&e))?;
            rows.map(|row| row.map_err(|e| unavailable(&e)).and_then(ServerRow::decode)).collect()
        })
    }

    fn add_server(&self, account: NewServerAccount) -> BoxFuture<'static, Result<ServerAccount, StoreError>> {
        self.run(move |connection| {
            let (auth_kind, auth_value) = auth_columns(&account.auth);
            connection
                .execute(
                    "INSERT INTO server_accounts (protocol, name, host, port, transport, username, root, auth_kind, auth_value)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        protocol_column(account.protocol),
                        account.name,
                        account.host,
                        account.port,
                        transport_column(account.transport),
                        account.username,
                        account.root,
                        auth_kind,
                        auth_value
                    ],
                )
                .map_err(|e| unavailable(&e))?;
            Ok(account.with_id(AccountId(connection.last_insert_rowid())))
        })
    }

    fn remove_server(&self, id: AccountId) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute("DELETE FROM server_accounts WHERE id = ?1", [id.0])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }
}
