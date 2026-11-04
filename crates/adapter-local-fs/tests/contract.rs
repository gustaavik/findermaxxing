//! `LocalFs` and `LocalTrash` satisfy the shared contract suites. The trash
//! cases use the real platform trash and restore everything they trash.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use adapter_local_fs::{LocalFs, LocalTrash, storage_path_from_native};
use domain::StoragePath;
use tempfile::TempDir;
use vfs_contract_tests::{VfsFixture, vfs_contract_tests};

struct LocalFixture {
    fs: LocalFs,
    dir: TempDir,
}

impl VfsFixture for LocalFixture {
    type Vfs = LocalFs;

    fn vfs(&self) -> &LocalFs {
        &self.fs
    }

    fn root(&self) -> StoragePath {
        storage_path_from_native(self.dir.path()).unwrap()
    }

    async fn create_dir(&self, relative: &str) {
        tokio::fs::create_dir(self.dir.path().join(relative)).await.unwrap();
    }

    async fn create_file(&self, relative: &str, contents: &[u8]) {
        tokio::fs::write(self.dir.path().join(relative), contents).await.unwrap();
    }

    async fn remove_file(&self, relative: &str) {
        tokio::fs::remove_file(self.dir.path().join(relative)).await.unwrap();
    }

    async fn read_file(&self, relative: &str) -> Option<Vec<u8>> {
        tokio::fs::read(self.dir.path().join(relative)).await.ok()
    }
}

struct TrashFixture {
    local: LocalFixture,
    trash: LocalTrash,
}

impl VfsFixture for TrashFixture {
    type Vfs = LocalTrash;

    fn vfs(&self) -> &LocalTrash {
        &self.trash
    }

    fn root(&self) -> StoragePath {
        self.local.root()
    }

    async fn create_dir(&self, relative: &str) {
        self.local.create_dir(relative).await;
    }

    async fn create_file(&self, relative: &str, contents: &[u8]) {
        self.local.create_file(relative, contents).await;
    }

    async fn remove_file(&self, relative: &str) {
        self.local.remove_file(relative).await;
    }

    async fn read_file(&self, relative: &str) -> Option<Vec<u8>> {
        self.local.read_file(relative).await
    }
}

async fn fixture() -> LocalFixture {
    LocalFixture { fs: LocalFs::new(tokio::runtime::Handle::current()), dir: tempfile::tempdir().unwrap() }
}

async fn trash_fixture() -> TrashFixture {
    TrashFixture { local: fixture().await, trash: LocalTrash::platform_default(tokio::runtime::Handle::current()) }
}

vfs_contract_tests!(fixture);

mod mutation {
    use super::fixture;
    vfs_contract_tests::mutation_contract_tests!(fixture);
}

mod stream {
    use super::fixture;
    vfs_contract_tests::stream_contract_tests!(fixture);
}

mod trash {
    use super::trash_fixture;
    vfs_contract_tests::trash_contract_tests!(trash_fixture);
}
