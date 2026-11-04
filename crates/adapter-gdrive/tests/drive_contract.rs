//! `DriveFs` against the Drive API double in `mock-cloud`: the shared
//! contract suites plus Drive's own quirks (duplicates, Google Docs, sign-in).
//! Each fixture starts its own mock, so every case begins with an empty drive.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use adapter_gdrive::{DriveFs, GoogleDriveEndpoints, GoogleDriveService, GoogleDriveSettings};
use application::CancelToken;
use application::ports::{DriveConnector, Lister, NewDriveAccount, Reader, Secret, UrlOpener};
use application::testing::InMemoryKeychain;
use domain::{AccountId, DriveProvider, PathSegment, StoragePath, VfsError};
use futures::future::BoxFuture;
use futures::{FutureExt, TryStreamExt};
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

fn service(cloud: &MockCloud) -> GoogleDriveService {
    let settings = GoogleDriveSettings {
        client_id: Some("test-client".into()),
        client_secret: Some("not-secret".into()),
        endpoints: GoogleDriveEndpoints {
            api: format!("{}/gdrive", cloud.base()),
            authorize: format!("{}/oauth/authorize", cloud.base()),
            token: format!("{}/oauth/token", cloud.base()),
        },
    };
    GoogleDriveService::new(
        tokio::runtime::Handle::current(),
        settings,
        Arc::new(Browser),
        Arc::new(InMemoryKeychain::default()),
    )
    .unwrap()
}

struct DriveFixture {
    cloud: MockCloud,
    fs: DriveFs,
}

impl DriveFixture {
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
        let parent = state.google.by_path(&parent, false).expect("parent exists");
        let mime = if folder { FOLDER_MIME } else { "application/octet-stream" };
        state.google.add(&parent, &name, folder, contents.to_vec(), mime);
    }
}

impl VfsFixture for DriveFixture {
    type Vfs = DriveFs;

    fn vfs(&self) -> &DriveFs {
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
        if let Some(id) = state.google.by_path(relative, false) {
            state.google.remove_tree(&id);
        }
    }

    async fn read_file(&self, relative: &str) -> Option<Vec<u8>> {
        let state = self.cloud.state();
        let item = state.google.get(&state.google.by_path(relative, false)?)?;
        (!item.folder).then(|| item.content.clone())
    }
}

async fn fixture() -> DriveFixture {
    let cloud = MockCloud::start().await;
    let service = service(&cloud);
    let account = NewDriveAccount { provider: DriveProvider::GoogleDrive, name: "Drive".into(), email: String::new() }
        .with_id(AccountId(1));
    service.connect(&account, Secret::new(cloud.issue_refresh_token())).await.unwrap();
    DriveFixture { fs: service.open_account(account.id).unwrap(), cloud }
}

mod listing {
    use super::fixture;
    vfs_contract_tests::lister_contract_tests!(fixture);
}

mod mutation {
    use super::fixture;
    vfs_contract_tests::mutation_contract_tests!(fixture);
}

mod trash {
    use super::fixture;
    vfs_contract_tests::trash_contract_tests!(fixture);
}

mod stream {
    use super::fixture;
    vfs_contract_tests::stream_contract_tests!(fixture);
}

mod resume {
    use super::fixture;
    vfs_contract_tests::resume_contract_tests!(fixture);
}

fn at(fixture: &DriveFixture, name: &str) -> StoragePath {
    fixture.root().join(PathSegment::new(name).unwrap())
}

async fn names(fixture: &DriveFixture) -> Vec<String> {
    let batches: Vec<_> = fixture.fs.list(&fixture.root(), CancelToken::never()).try_collect().await.unwrap();
    batches.into_iter().flatten().map(|e| e.name().display_name().into_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicates_are_shown_numbered_and_each_stays_reachable() {
    let f = fixture().await;
    f.create_file("report.pdf", b"first").await;
    f.create_file("report.pdf", b"second").await;
    assert_eq!(names(&f).await, ["report.pdf", "report (2).pdf"]);
    let read = |name: &'static str| {
        let fs = f.fs.clone();
        let path = at(&f, name);
        async move {
            let chunks: Vec<Vec<u8>> = fs.read(&path, CancelToken::never()).await.unwrap().try_collect().await.unwrap();
            chunks.concat()
        }
    };
    assert_eq!(read("report.pdf").await, b"first");
    assert_eq!(read("report (2).pdf").await, b"second");
    assert_eq!(
        f.cloud.state().google.children("root").iter().filter(|i| i.name == "report.pdf").count(),
        2,
        "nothing renamed on Drive"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn google_docs_export_to_office_formats() {
    let f = fixture().await;
    f.cloud.add_google_doc("root", "Budget", "spreadsheet");
    assert_eq!(names(&f).await, ["Budget.xlsx"]);
    let entry = f.fs.stat(&at(&f, "Budget.xlsx")).await.unwrap();
    assert_eq!(entry.metadata().size, None, "an export's size isn't known in advance");
    let chunks: Vec<Vec<u8>> =
        f.fs.read(&at(&f, "Budget.xlsx"), CancelToken::never()).await.unwrap().try_collect().await.unwrap();
    assert_eq!(
        String::from_utf8(chunks.concat()).unwrap(),
        "EXPORT:Budget:application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_through_the_browser_finds_the_user() {
    let cloud = MockCloud::start().await;
    let signed_in = service(&cloud).sign_in(CancelToken::never()).await.unwrap();
    assert_eq!((signed_in.email.as_str(), signed_in.display_name.as_str()), ("mock@gmail.com", "Mock User"));
    assert!(cloud.state().oauth.last_scope.contains("auth/drive"));
}

#[tokio::test(flavor = "multi_thread")]
async fn builds_without_a_client_id_say_so() {
    let cloud = MockCloud::start().await;
    let unconfigured = GoogleDriveService::new(
        tokio::runtime::Handle::current(),
        GoogleDriveSettings { client_id: None, ..GoogleDriveSettings::default() },
        Arc::new(Browser),
        Arc::new(InMemoryKeychain::default()),
    )
    .unwrap();
    let _ = cloud;
    assert_eq!(
        unconfigured.sign_in(CancelToken::never()).await,
        Err(application::ports::SignInError::NotConfigured(DriveProvider::GoogleDrive))
    );
}

const PROVIDER: DriveProvider = DriveProvider::GoogleDrive;
const TOKEN_VARIABLE: &str = "FM_LIVE_GOOGLE_REFRESH_TOKEN";

fn live_service() -> GoogleDriveService {
    let settings = GoogleDriveSettings {
        client_id: std::env::var("FM_GOOGLE_CLIENT_ID").ok(),
        client_secret: std::env::var("FM_GOOGLE_CLIENT_SECRET").ok(),
        endpoints: GoogleDriveEndpoints::default(),
    };
    GoogleDriveService::new(
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
