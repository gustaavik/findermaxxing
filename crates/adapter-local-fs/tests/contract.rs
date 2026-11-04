//! `LocalFs` satisfies the shared `Lister` + `Watcher` contract.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use adapter_local_fs::{LocalFs, storage_path_from_native};
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
}

async fn fixture() -> LocalFixture {
    LocalFixture { fs: LocalFs::new(tokio::runtime::Handle::current()), dir: tempfile::tempdir().unwrap() }
}

vfs_contract_tests!(fixture);
