//! `SmbFs` against a real Samba server (`dockurr/samba` in Docker). Ignored
//! by default; run with
//!
//! ```sh
//! docker run -d --rm --name findermaxxing-smb -p 127.0.0.1:1445:445 \
//!     -e USER=fmtest -e PASS=fmtest-secret -e NAME=Data dockurr/samba
//! cargo test -p adapter-smb --test smb_contract -- --ignored
//! ```
//!
//! Each fixture gets its own folder in the `Data` share, which is the
//! account's root. Setup and inspection go through a second account on the share.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use adapter_smb::{SmbFs, SmbService};
use application::CancelToken;
use application::ports::{
    ConnectError, FolderMaker, NewServerAccount, Reader, Remover, Secret, ServerConnector, Writer,
};
use domain::{AccountId, HostTrust, PathSegment, ServerAccount, ServerAuth, ServerProtocol, StoragePath, Transport};
use futures::TryStreamExt;
use vfs_contract_tests::VfsFixture;

const HOST: &str = "127.0.0.1";
const PORT_VARIABLE: &str = "FM_SMB_TEST_PORT";
const DEFAULT_PORT: &str = "1445";
const USER: &str = "fmtest";
const PASSWORD: &str = "fmtest-secret";
const UPLOAD: &str = "Data";

static FOLDERS: AtomicU64 = AtomicU64::new(0);
static ACCOUNTS: AtomicI64 = AtomicI64::new(1);

fn account(root: &str) -> ServerAccount {
    NewServerAccount {
        protocol: ServerProtocol::Smb,
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

async fn connected(service: &SmbService, account: &ServerAccount) -> SmbFs {
    let secret = Some(Secret::new(PASSWORD.into()));
    if let Err(error) = service.connect(account, secret, HostTrust::KnownOnly).await {
        panic!("SMB test server reachable (see module docs): {error}");
    }
    service.open_account(account.id).unwrap()
}

struct SmbFixture {
    fs: SmbFs,
    admin: SmbFs,
    folder: StoragePath,
}

impl SmbFixture {
    fn at(&self, relative: &str) -> StoragePath {
        relative
            .split('/')
            .filter(|part| !part.is_empty())
            .fold(self.folder.clone(), |path, part| path.join(PathSegment::new(part).unwrap()))
    }
}

impl VfsFixture for SmbFixture {
    type Vfs = SmbFs;

    fn vfs(&self) -> &SmbFs {
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

async fn fixture() -> SmbFixture {
    let service = SmbService::new(tokio::runtime::Handle::current()).unwrap();
    let admin = connected(&service, &account(UPLOAD)).await;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let name = format!("fm-test-{nanos}-{}", FOLDERS.fetch_add(1, Ordering::SeqCst));
    let folder = admin.root().join(PathSegment::new(&name).unwrap());
    admin.create_folder(&folder).await.unwrap();
    let fs = connected(&service, &account(&format!("{UPLOAD}/{name}"))).await;
    SmbFixture { fs, admin, folder }
}

mod listing {
    use super::fixture;
    vfs_contract_tests::lister_contract_tests!(fixture, #[ignore = "needs an SMB server: see module docs"]);
}

mod mutation {
    use super::fixture;
    vfs_contract_tests::mutation_contract_tests!(fixture, #[ignore = "needs an SMB server: see module docs"]);
}

mod stream {
    use super::fixture;
    vfs_contract_tests::stream_contract_tests!(fixture, #[ignore = "needs an SMB server: see module docs"]);
}

mod resume {
    use super::fixture;
    vfs_contract_tests::resume_contract_tests!(fixture, #[ignore = "needs an SMB server: see module docs"]);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an SMB server: see module docs"]
async fn wrong_passwords_and_missing_shares_fail() {
    let service = SmbService::new(tokio::runtime::Handle::current()).unwrap();
    let wrong = service.connect(&account(UPLOAD), Some(Secret::new("nope".into())), HostTrust::KnownOnly).await;
    assert_eq!(wrong, Err(ConnectError::Failed(domain::VfsError::PermissionDenied)));
    let missing =
        service.connect(&account("NoSuchShare"), Some(Secret::new(PASSWORD.into())), HostTrust::KnownOnly).await;
    assert!(matches!(missing, Err(ConnectError::Failed(_))), "{missing:?}");
}
