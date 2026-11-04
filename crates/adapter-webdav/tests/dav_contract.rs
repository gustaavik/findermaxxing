//! `DavFs` against a real WebDAV server (Apache `mod_dav` in Docker; any
//! server works). Ignored by default; run with
//!
//! ```sh
//! docker run -d --rm --name findermaxxing-dav -p 127.0.0.1:8090:80 \
//!     -e AUTH_TYPE=Basic -e USERNAME=fmtest -e PASSWORD=fmtest-secret bytemark/webdav
//! cargo test -p adapter-webdav --test dav_contract -- --ignored
//! ```
//!
//! Each fixture works in its own folder, which is the account's root.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use adapter_webdav::{DavFs, WebDavService};
use application::ports::{NewServerAccount, Secret, ServerConnector};
use domain::{AccountId, HostTrust, ServerAuth, ServerProtocol, StoragePath, Transport};
use reqwest::Method;
use vfs_contract_tests::VfsFixture;

const HOST_VARIABLE: &str = "FM_DAV_TEST_HOST";
const DEFAULT_HOST: &str = "127.0.0.1";
const PORT_VARIABLE: &str = "FM_DAV_TEST_PORT";
const DEFAULT_PORT: &str = "8090";
const USER: &str = "fmtest";
const PASSWORD: &str = "fmtest-secret";

static FOLDERS: AtomicU64 = AtomicU64::new(0);
static ACCOUNTS: AtomicU64 = AtomicU64::new(1);

fn setting(variable: &str, default: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| default.to_owned())
}

struct DavFixture {
    fs: DavFs,
    raw: reqwest::Client,
    base: String,
    root: StoragePath,
}

impl DavFixture {
    fn url(&self, relative: &str) -> String {
        format!("{}{}", self.base, relative.trim_matches('/'))
    }

    async fn raw(&self, method: &[u8], url: String) -> reqwest::Response {
        let method = Method::from_bytes(method).unwrap();
        self.raw.request(method, url).basic_auth(USER, Some(PASSWORD)).send().await.unwrap()
    }
}

impl VfsFixture for DavFixture {
    type Vfs = DavFs;

    fn vfs(&self) -> &DavFs {
        &self.fs
    }

    fn root(&self) -> StoragePath {
        self.root.clone()
    }

    async fn create_dir(&self, relative: &str) {
        assert!(self.raw(b"MKCOL", format!("{}/", self.url(relative))).await.status().is_success());
    }

    async fn create_file(&self, relative: &str, contents: &[u8]) {
        let request = self.raw.put(self.url(relative)).basic_auth(USER, Some(PASSWORD)).body(contents.to_vec());
        assert!(request.send().await.unwrap().status().is_success());
    }

    async fn remove_file(&self, relative: &str) {
        self.raw(b"DELETE", self.url(relative)).await;
    }

    async fn read_file(&self, relative: &str) -> Option<Vec<u8>> {
        let response = self.raw(b"GET", self.url(relative)).await;
        response.status().is_success().then_some(())?;
        Some(response.bytes().await.unwrap().to_vec())
    }
}

async fn fixture() -> DavFixture {
    let (host, port) = (setting(HOST_VARIABLE, DEFAULT_HOST), setting(PORT_VARIABLE, DEFAULT_PORT));
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let folder = format!("fm-test-{nanos}-{}", FOLDERS.fetch_add(1, Ordering::SeqCst));
    let raw = reqwest::Client::new();
    let base = format!("http://{host}:{port}/{folder}/");
    let created = raw
        .request(Method::from_bytes(b"MKCOL").unwrap(), &base)
        .basic_auth(USER, Some(PASSWORD))
        .send()
        .await
        .expect("WebDAV test server reachable (see module docs)");
    assert!(created.status().is_success(), "creating the test folder: {}", created.status());

    let service = WebDavService::new(tokio::runtime::Handle::current()).unwrap();
    let account = NewServerAccount {
        protocol: ServerProtocol::WebDav,
        name: "test".into(),
        host,
        port: Some(port.parse().unwrap()),
        transport: Transport::Plain,
        username: USER.into(),
        root: folder,
        auth: ServerAuth::Password,
    }
    .with_id(AccountId(i64::try_from(ACCOUNTS.fetch_add(1, Ordering::SeqCst)).unwrap()));
    service.connect(&account, Some(Secret::new(PASSWORD.into())), HostTrust::KnownOnly).await.unwrap();
    let root = StoragePath::root(account.mount().unwrap());
    DavFixture { fs: service.open_account(account.id).unwrap(), raw, base, root }
}

mod listing {
    use super::fixture;
    vfs_contract_tests::lister_contract_tests!(fixture, #[ignore = "needs a WebDAV server: see module docs"]);
}

mod mutation {
    use super::fixture;
    vfs_contract_tests::mutation_contract_tests!(fixture, #[ignore = "needs a WebDAV server: see module docs"]);
}

mod stream {
    use super::fixture;
    vfs_contract_tests::stream_contract_tests!(fixture, #[ignore = "needs a WebDAV server: see module docs"]);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a WebDAV server: see module docs"]
async fn wrong_passwords_are_permission_denied() {
    let service = WebDavService::new(tokio::runtime::Handle::current()).unwrap();
    let account = NewServerAccount {
        protocol: ServerProtocol::WebDav,
        name: "wrong".into(),
        host: setting(HOST_VARIABLE, DEFAULT_HOST),
        port: Some(setting(PORT_VARIABLE, DEFAULT_PORT).parse().unwrap()),
        transport: Transport::Plain,
        username: USER.into(),
        root: String::new(),
        auth: ServerAuth::Password,
    }
    .with_id(AccountId(999));
    let result = service.connect(&account, Some(Secret::new("nope".into())), HostTrust::KnownOnly).await;
    assert_eq!(result, Err(application::ports::ConnectError::Failed(domain::VfsError::PermissionDenied)));
}
