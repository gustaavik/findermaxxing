//! `DropboxFs` against the Dropbox API double in `mock-cloud`: the shared
//! contract suites plus sign-in. Each fixture starts its own mock, so every
//! case begins with an empty Dropbox.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use adapter_dropbox::{DropboxEndpoints, DropboxFs, DropboxService, DropboxSettings};
use application::CancelToken;
use application::ports::{DriveConnector, NewDriveAccount, Secret, UrlOpener};
use application::testing::InMemoryKeychain;
use domain::{AccountId, DriveProvider, PathSegment, StoragePath, VfsError};
use futures::future::BoxFuture;
use futures::{FutureExt, TryStreamExt};
use infra_oauth::Redirect;
use mock_cloud::MockCloud;
use vfs_contract_tests::VfsFixture;

const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

struct Browser;

impl UrlOpener for Browser {
    fn open_url(&self, url: &str) -> BoxFuture<'static, Result<(), VfsError>> {
        let url = url.to_owned();
        tokio::spawn(async move { MockCloud::visit_as_browser(&url).await });
        futures::future::ready(Ok(())).boxed()
    }
}

fn service(cloud: &MockCloud) -> DropboxService {
    let settings = DropboxSettings {
        client_id: Some("test-client".into()),
        endpoints: DropboxEndpoints {
            rpc: format!("{}/dropbox/api", cloud.base()),
            content: format!("{}/dropbox/content", cloud.base()),
            authorize: format!("{}/oauth/authorize", cloud.base()),
            token: format!("{}/oauth/token", cloud.base()),
        },
        redirect: Redirect::AnyPort { host: "127.0.0.1" },
    };
    DropboxService::new(
        tokio::runtime::Handle::current(),
        settings,
        Arc::new(Browser),
        Arc::new(InMemoryKeychain::default()),
    )
    .unwrap()
}

struct DropboxFixture {
    cloud: MockCloud,
    fs: DropboxFs,
}

impl DropboxFixture {
    fn parent_and_name(relative: &str) -> (String, String) {
        let trimmed = relative.trim_matches('/');
        match trimmed.rsplit_once('/') {
            Some((parent, name)) => (parent.to_owned(), name.to_owned()),
            None => (String::new(), trimmed.to_owned()),
        }
    }

    fn add(&self, relative: &str, folder: bool, contents: &[u8]) {
        let (parent, name) = Self::parent_and_name(relative);
        let mut state = self.cloud.state();
        let parent = state.dropbox.by_path(&parent, true).expect("parent exists");
        let mime = if folder { FOLDER_MIME } else { "application/octet-stream" };
        state.dropbox.add(&parent, &name, folder, contents.to_vec(), mime);
    }
}

impl VfsFixture for DropboxFixture {
    type Vfs = DropboxFs;

    fn vfs(&self) -> &DropboxFs {
        &self.fs
    }

    fn root(&self) -> StoragePath {
        self.fs.root().clone()
    }

    async fn create_dir(&self, relative: &str) {
        self.add(relative, true, b"");
    }

    async fn create_file(&self, relative: &str, contents: &[u8]) {
        self.add(relative, false, contents);
    }

    async fn remove_file(&self, relative: &str) {
        let mut state = self.cloud.state();
        if let Some(id) = state.dropbox.by_path(relative, true) {
            state.dropbox.remove_tree(&id);
        }
    }

    async fn read_file(&self, relative: &str) -> Option<Vec<u8>> {
        let state = self.cloud.state();
        let item = state.dropbox.get(&state.dropbox.by_path(relative, true)?)?;
        (!item.folder).then(|| item.content.clone())
    }
}

async fn fixture() -> DropboxFixture {
    let cloud = MockCloud::start().await;
    let service = service(&cloud);
    let account = NewDriveAccount { provider: DriveProvider::Dropbox, name: "Drive".into(), email: String::new() }
        .with_id(AccountId(1));
    service.connect(&account, Secret::new(cloud.issue_refresh_token())).await.unwrap();
    DropboxFixture { fs: service.open_account(account.id).unwrap(), cloud }
}

mod listing {
    use super::fixture;
    vfs_contract_tests::lister_contract_tests!(fixture);
}

mod mutation {
    use super::fixture;
    vfs_contract_tests::mutation_contract_tests!(fixture);
}

mod stream {
    use super::fixture;
    vfs_contract_tests::stream_contract_tests!(fixture);
}

mod resume {
    use super::fixture;
    vfs_contract_tests::resume_contract_tests!(fixture);
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_through_the_browser_finds_the_user() {
    let cloud = MockCloud::start().await;
    let signed_in = service(&cloud).sign_in(CancelToken::never()).await.unwrap();
    assert_eq!((signed_in.email.as_str(), signed_in.display_name.as_str()), ("mock@dropbox.test", "Mock Dropboxer"));
}

#[tokio::test(flavor = "multi_thread")]
async fn names_outside_ascii_survive_the_header_arguments() {
    use application::ports::{Reader, Writer};
    let f = fixture().await;
    let path = f.root().join(PathSegment::new("Café 🎉.txt").unwrap());
    let mut sink = f.fs.create(&path, None).await.unwrap();
    sink.write(b"bonjour".to_vec()).await.unwrap();
    sink.commit().await.unwrap();
    let chunks: Vec<Vec<u8>> = f.fs.read(&path, CancelToken::never()).await.unwrap().try_collect().await.unwrap();
    assert_eq!(chunks.concat(), b"bonjour");
}

const PROVIDER: DriveProvider = DriveProvider::Dropbox;
const TOKEN_VARIABLE: &str = "FM_LIVE_DROPBOX_REFRESH_TOKEN";

fn live_service() -> DropboxService {
    let settings =
        DropboxSettings { client_id: std::env::var("FM_DROPBOX_APP_KEY").ok(), ..DropboxSettings::default() };
    DropboxService::new(
        tokio::runtime::Handle::current(),
        settings,
        Arc::new(SystemBrowser),
        Arc::new(InMemoryKeychain::default()),
    )
    .unwrap()
}

/// A real browser, for the live test's first sign-in.
struct SystemBrowser;

impl UrlOpener for SystemBrowser {
    fn open_url(&self, url: &str) -> BoxFuture<'static, Result<(), VfsError>> {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let result = std::process::Command::new(opener)
            .arg(url)
            .spawn()
            .map(drop)
            .map_err(|e| VfsError::Io { message: e.to_string() });
        futures::future::ready(result).boxed()
    }
}

/// Against the real service, in a throwaway folder that is deleted again.
/// Needs this build's OAuth client in `CLIENT_VARIABLES`. Without
/// `TOKEN_VARIABLE` it signs in through the browser first and prints a
/// refresh token to reuse.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to the real service; see docs/oauth-setup.md"]
async fn live_round_trip() {
    use application::ports::{FolderMaker, Lister, Reader, Remover, Writer};
    let service = live_service();
    let refresh = match std::env::var(TOKEN_VARIABLE) {
        Ok(token) => token,
        Err(_) => {
            let signed_in = service.sign_in(CancelToken::never()).await.expect("browser sign-in");
            println!("{TOKEN_VARIABLE}={}  (signed in as {})", signed_in.refresh_token.expose(), signed_in.email);
            signed_in.refresh_token.expose().to_owned()
        }
    };
    let account =
        NewDriveAccount { provider: PROVIDER, name: "live".into(), email: String::new() }.with_id(AccountId(1));
    service.connect(&account, Secret::new(refresh)).await.expect("connect");
    let fs = service.open_account(account.id).unwrap();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let folder = fs.root().join(PathSegment::new(&format!("FinderMaxxing live test {nanos}")).unwrap());
    fs.create_folder(&folder).await.unwrap();
    let file = folder.join(PathSegment::new("hello é.txt").unwrap());
    let mut sink = fs.create(&file, Some(domain::ByteSize(5))).await.unwrap();
    sink.write(b"hello".to_vec()).await.unwrap();
    sink.commit().await.unwrap();
    let listed: Vec<_> = fs.list(&folder, CancelToken::never()).try_collect().await.unwrap();
    assert_eq!(listed.concat().len(), 1);
    let chunks: Vec<Vec<u8>> = fs.read(&file, CancelToken::never()).await.unwrap().try_collect().await.unwrap();
    assert_eq!(chunks.concat(), b"hello");
    fs.remove(&folder, CancelToken::never()).await.unwrap();
}
