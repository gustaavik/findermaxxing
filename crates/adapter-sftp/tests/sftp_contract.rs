//! `SftpFs` against a real OpenSSH server (`atmoz/sftp` in Docker). Ignored
//! by default; run with
//!
//! ```sh
//! docker run -d --rm --name findermaxxing-sftp -p 127.0.0.1:2222:22 atmoz/sftp fmtest:fmtest-secret:::upload
//! cargo test -p adapter-sftp --test sftp_contract -- --ignored
//! ```
//!
//! Each fixture gets its own folder below `upload/`, which is the account's
//! root. Setup and inspection go through a second account rooted at `upload/`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use adapter_sftp::{SftpFs, SftpService};
use application::CancelToken;
use application::ports::{
    ConnectError, FolderMaker, NewServerAccount, Reader, Remover, Secret, ServerConnector, Writer,
};
use domain::{AccountId, HostTrust, PathSegment, ServerAccount, ServerAuth, ServerProtocol, StoragePath, Transport};
use futures::TryStreamExt;
use vfs_contract_tests::VfsFixture;

const HOST: &str = "127.0.0.1";
const PORT_VARIABLE: &str = "FM_SFTP_TEST_PORT";
const DEFAULT_PORT: &str = "2222";
const USER: &str = "fmtest";
const PASSWORD: &str = "fmtest-secret";
const UPLOAD: &str = "upload";

static FOLDERS: AtomicU64 = AtomicU64::new(0);
static ACCOUNTS: AtomicI64 = AtomicI64::new(1);

fn account(root: &str) -> ServerAccount {
    NewServerAccount {
        protocol: ServerProtocol::Sftp,
        name: "test".into(),
        host: HOST.into(),
        port: Some(std::env::var(PORT_VARIABLE).unwrap_or_else(|_| DEFAULT_PORT.into()).parse().unwrap()),
        transport: Transport::Tls,
        username: USER.into(),
        root: root.into(),
        auth: ServerAuth::Password,
    }
    .with_id(AccountId(ACCOUNTS.fetch_add(1, Ordering::SeqCst)))
}

/// Connects, accepting the server's key the way the UI does: the first
/// attempt reports it, the second accepts exactly that key.
async fn connected(service: &SftpService, account: &ServerAccount) -> SftpFs {
    let secret = || Some(Secret::new(PASSWORD.into()));
    match service.connect(account, secret(), HostTrust::KnownOnly).await {
        Ok(()) => {}
        Err(ConnectError::UnknownHost(key)) => {
            service.connect(account, secret(), HostTrust::Accept(key)).await.unwrap();
        }
        Err(error) => panic!("SFTP test server reachable (see module docs): {error}"),
    }
    service.open_account(account.id).unwrap()
}

struct SftpFixture {
    fs: SftpFs,
    admin: SftpFs,
    folder: StoragePath,
    _known_hosts: tempfile::TempDir,
}

impl SftpFixture {
    fn at(&self, relative: &str) -> StoragePath {
        relative
            .split('/')
            .filter(|part| !part.is_empty())
            .fold(self.folder.clone(), |path, part| path.join(PathSegment::new(part).unwrap()))
    }
}

impl VfsFixture for SftpFixture {
    type Vfs = SftpFs;

    fn vfs(&self) -> &SftpFs {
        &self.fs
    }

    fn root(&self) -> StoragePath {
        self.fs.root().clone()
    }

    async fn create_dir(&self, relative: &str) {
        self.admin.create_folder(&self.at(relative)).await.unwrap();
    }

    async fn create_file(&self, relative: &str, contents: &[u8]) {
        let mut sink = self.admin.create(&self.at(relative), None).await.unwrap();
        sink.write(contents.to_vec()).await.unwrap();
        sink.commit().await.unwrap();
    }

    async fn remove_file(&self, relative: &str) {
        let _ = self.admin.remove(&self.at(relative), CancelToken::never()).await;
    }

    async fn read_file(&self, relative: &str) -> Option<Vec<u8>> {
        let chunks: Vec<Vec<u8>> =
            self.admin.read(&self.at(relative), CancelToken::never()).await.ok()?.try_collect().await.ok()?;
        Some(chunks.concat())
    }
}

async fn fixture() -> SftpFixture {
    let known_hosts = tempfile::tempdir().unwrap();
    let service = SftpService::new(tokio::runtime::Handle::current(), known_hosts.path().join("known_hosts")).unwrap();
    let admin = connected(&service, &account(UPLOAD)).await;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let name = format!("fm-test-{nanos}-{}", FOLDERS.fetch_add(1, Ordering::SeqCst));
    let folder = admin.root().join(PathSegment::new(&name).unwrap());
    admin.create_folder(&folder).await.unwrap();
    let fs = connected(&service, &account(&format!("{UPLOAD}/{name}"))).await;
    SftpFixture { fs, admin, folder, _known_hosts: known_hosts }
}

mod listing {
    use super::fixture;
    vfs_contract_tests::lister_contract_tests!(fixture, #[ignore = "needs an SFTP server: see module docs"]);
}

mod mutation {
    use super::fixture;
    vfs_contract_tests::mutation_contract_tests!(fixture, #[ignore = "needs an SFTP server: see module docs"]);
}

mod permissions {
    use super::fixture;
    vfs_contract_tests::permission_contract_tests!(fixture, #[ignore = "needs an SFTP server: see module docs"]);
}

mod stream {
    use super::fixture;
    vfs_contract_tests::stream_contract_tests!(fixture, #[ignore = "needs an SFTP server: see module docs"]);
}

mod resume {
    use super::fixture;
    vfs_contract_tests::resume_contract_tests!(fixture, #[ignore = "needs an SFTP server: see module docs"]);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an SFTP server: see module docs"]
async fn unknown_keys_need_consent_and_wrong_passwords_fail() {
    let known_hosts = tempfile::tempdir().unwrap();
    let service = SftpService::new(tokio::runtime::Handle::current(), known_hosts.path().join("known_hosts")).unwrap();
    let account = account(UPLOAD);
    let secret = || Some(Secret::new(PASSWORD.into()));
    // Unless ~/.ssh/known_hosts already lists the test server, the key is new.
    if let Err(ConnectError::UnknownHost(key)) = service.connect(&account, secret(), HostTrust::KnownOnly).await {
        assert!(key.fingerprint.starts_with("SHA256:"), "{key:?}");
        let mut wrong = key.clone();
        wrong.fingerprint = "SHA256:not-the-key".into();
        assert_eq!(
            service.connect(&account, secret(), HostTrust::Accept(wrong)).await,
            Err(ConnectError::UnknownHost(key.clone())),
            "accepting a different key trusts nothing"
        );
        service.connect(&account, secret(), HostTrust::Accept(key)).await.unwrap();
    }
    service.connect(&account, secret(), HostTrust::KnownOnly).await.unwrap();
    let wrong = service.connect(&account, Some(Secret::new("nope".into())), HostTrust::KnownOnly).await;
    assert_eq!(wrong, Err(ConnectError::Failed(domain::VfsError::PermissionDenied)));
}
