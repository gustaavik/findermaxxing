//! The in-memory fake used by use-case tests must honour the same contract
//! as real adapters, otherwise those tests prove nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use application::testing::FakeVfs;
use domain::StoragePath;
use vfs_contract_tests::{VfsFixture, vfs_contract_tests};

const ROOT: &str = "/work";

struct FakeFixture {
    fake: FakeVfs,
}

impl VfsFixture for FakeFixture {
    type Vfs = FakeVfs;

    fn vfs(&self) -> &FakeVfs {
        &self.fake
    }

    fn root(&self) -> StoragePath {
        self.fake.path(ROOT)
    }

    async fn create_dir(&self, relative: &str) {
        self.fake.add_dir(&format!("{ROOT}/{relative}"));
    }

    async fn create_file(&self, relative: &str, contents: &[u8]) {
        self.fake.add_file_with(&format!("{ROOT}/{relative}"), contents);
    }

    async fn remove_file(&self, relative: &str) {
        self.fake.remove(&format!("{ROOT}/{relative}"));
    }

    async fn read_file(&self, relative: &str) -> Option<Vec<u8>> {
        self.fake.contents(&format!("{ROOT}/{relative}"))
    }
}

async fn fixture() -> FakeFixture {
    let fake = FakeVfs::new();
    fake.add_dir(ROOT);
    FakeFixture { fake }
}

vfs_contract_tests!(fixture);

mod mutation {
    use super::fixture;
    vfs_contract_tests::mutation_contract_tests!(fixture);
}

mod permissions {
    use super::fixture;
    vfs_contract_tests::permission_contract_tests!(fixture);
}

mod tags {
    use super::fixture;
    vfs_contract_tests::tag_contract_tests!(fixture);
}

mod trash {
    use super::fixture;
    vfs_contract_tests::trash_contract_tests!(fixture);
}

mod stream {
    use super::fixture;
    vfs_contract_tests::stream_contract_tests!(fixture);
    vfs_contract_tests::resume_contract_tests!(fixture);
}
