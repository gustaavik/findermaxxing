//! `BucketFs` against a real S3-compatible server (SeaweedFS or MinIO in
//! Docker; AWS works too). Ignored by default; run with
//!
//! ```sh
//! docker run -d --rm --name findermaxxing-s3 -p 127.0.0.1:8333:8333 chrislusf/seaweedfs \
//!     server -s3 -dir=/data -volume.max=1000 -master.volumeSizeLimitMB=64
//! FM_S3_TEST_ENDPOINT=http://127.0.0.1:8333 cargo test -p adapter-s3 --test s3_contract -- --ignored
//! ```
//!
//! Each fixture creates its own bucket, so cases run in parallel.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use adapter_s3::{BucketFs, S3Service};
use application::ports::{S3Connector, Secret};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use domain::{AccountId, Addressing, BucketName, S3Account, S3Auth, S3Location, StoragePath};
use vfs_contract_tests::VfsFixture;

const ENDPOINT_VARIABLE: &str = "FM_S3_TEST_ENDPOINT";
const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8333";
const ACCESS_KEY_VARIABLE: &str = "FM_S3_TEST_ACCESS_KEY";
const SECRET_KEY_VARIABLE: &str = "FM_S3_TEST_SECRET_KEY";
const DEFAULT_KEY: &str = "fmtest";
const DEFAULT_SECRET: &str = "fmtest-secret-key";
const REGION: &str = "us-east-1";
const ACCOUNT: AccountId = AccountId(1);

static BUCKETS: AtomicU64 = AtomicU64::new(0);

fn setting(variable: &str, default: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| default.to_owned())
}

struct S3Fixture {
    bucket: BucketFs,
    raw: Client,
    name: String,
    root: StoragePath,
}

impl S3Fixture {
    fn key(relative: &str) -> String {
        relative.trim_matches('/').to_owned()
    }
}

impl VfsFixture for S3Fixture {
    type Vfs = BucketFs;

    fn vfs(&self) -> &BucketFs {
        &self.bucket
    }

    fn root(&self) -> StoragePath {
        self.root.clone()
    }

    async fn create_dir(&self, relative: &str) {
        let key = format!("{}/", Self::key(relative));
        self.raw.put_object().bucket(&self.name).key(key).body(ByteStream::from_static(b"")).send().await.unwrap();
    }

    async fn create_file(&self, relative: &str, contents: &[u8]) {
        let body = ByteStream::from(contents.to_vec());
        self.raw.put_object().bucket(&self.name).key(Self::key(relative)).body(body).send().await.unwrap();
    }

    async fn remove_file(&self, relative: &str) {
        self.raw.delete_object().bucket(&self.name).key(Self::key(relative)).send().await.unwrap();
    }

    async fn read_file(&self, relative: &str) -> Option<Vec<u8>> {
        let output = self.raw.get_object().bucket(&self.name).key(Self::key(relative)).send().await.ok()?;
        Some(output.body.collect().await.ok()?.to_vec())
    }
}

async fn fixture() -> S3Fixture {
    let endpoint = setting(ENDPOINT_VARIABLE, DEFAULT_ENDPOINT);
    let (key, secret) = (setting(ACCESS_KEY_VARIABLE, DEFAULT_KEY), setting(SECRET_KEY_VARIABLE, DEFAULT_SECRET));
    let config = aws_sdk_s3::config::Builder::new()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(REGION))
        .endpoint_url(&endpoint)
        .credentials_provider(Credentials::new(&key, &secret, None, None, "tests"))
        .force_path_style(true)
        .build();
    let raw = Client::from_conf(config);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let name = format!("fm-test-{nanos}-{}", BUCKETS.fetch_add(1, Ordering::SeqCst));
    raw.create_bucket().bucket(&name).send().await.expect("S3 test server reachable (see module docs)");

    let cache = tempfile::tempdir().unwrap().keep();
    let service = S3Service::new(tokio::runtime::Handle::current(), cache).unwrap();
    let account = S3Account {
        id: ACCOUNT,
        name: "test".into(),
        endpoint: Some(endpoint),
        region: Some(REGION.into()),
        auth: S3Auth::AccessKey { access_key_id: key },
        addressing: Addressing::PathStyle,
        skip_cost_warnings: false,
        extra_buckets: Vec::new(),
    };
    service.connect(&account, Some(Secret::new(secret))).await.unwrap();
    let location = S3Location { account: ACCOUNT, bucket: BucketName::new(&name).unwrap() };
    let root = StoragePath::root(location.mount().unwrap());
    S3Fixture { bucket: service.open_bucket(&location).unwrap(), raw, name, root }
}

mod listing {
    use super::fixture;
    vfs_contract_tests::lister_contract_tests!(fixture, #[ignore = "needs an S3 server: FM_S3_TEST_ENDPOINT"]);
}

mod mutation {
    use super::fixture;
    vfs_contract_tests::mutation_contract_tests!(fixture, #[ignore = "needs an S3 server: FM_S3_TEST_ENDPOINT"]);
}

mod stream {
    use super::fixture;
    vfs_contract_tests::stream_contract_tests!(fixture, #[ignore = "needs an S3 server: FM_S3_TEST_ENDPOINT"]);
}
