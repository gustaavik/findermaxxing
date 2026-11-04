//! Copy and move between providers, cost confirmation, providers without a
//! Trash, remote accounts and favourites: all against in-memory fakes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use application::operations::{AcceptCosts, JobContext, OperationError, OperationHistory};
use application::ports::{AccountStore, MountResolver, NewS3Account, S3Connector, Secret};
use application::testing::{
    FakeS3Connector, FakeVfs, InMemoryAccounts, InMemoryFavorites, InMemoryKeychain, RecordingObserver,
    RecordingOpener, ScriptedCosts, ScriptedResolver,
};
use application::use_cases::{
    AccountError, AccountServices, AddFavorite, AddS3Account, ConnectSavedAccounts, CopyEntries, ListFavorites,
    MoveEntries, OpenEntry, RedoOperation, RemoveFavorite, RemoveS3Account, TransferRequest, TrashEntries,
    UndoOperation,
};
use application::{CancelToken, ProviderRegistry};
use domain::{
    Addressing, ByteSize, ConflictChoice, ConflictResolution, ConflictScope, CostDecision, CostPolicy, CostReason,
    S3Auth, VfsError,
};
use futures_executor::block_on;

struct World {
    local: FakeVfs,
    remote: FakeVfs,
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl World {
    fn new() -> Self {
        Self::with_remote(FakeVfs::remote("remote", Some("eu-west-1")))
    }

    fn with_remote(remote: FakeVfs) -> Self {
        let local = FakeVfs::new();
        local.add_dir("/home");
        remote.add_dir("/bucket");
        let registry = ProviderRegistry::default()
            .with_provider(local.clone().into_factory())
            .and_then(|r| r.with_provider(remote.clone().into_factory()))
            .unwrap();
        Self { local, remote, mounts: Arc::new(registry), history: Arc::default() }
    }

    fn ctx(&self, choice: ConflictChoice) -> JobContext {
        JobContext {
            observer: Arc::new(RecordingObserver::default()),
            resolver: Arc::new(ScriptedResolver::new(vec![ConflictResolution {
                choice,
                scope: ConflictScope::ThisItem,
            }])),
            costs: Arc::new(AcceptCosts),
            cancel: CancelToken::never(),
        }
    }

    fn copy(&self) -> CopyEntries {
        CopyEntries::new(self.mounts.clone(), self.history.clone())
    }

    fn undo(&self) -> Result<(), OperationError> {
        let ctx = JobContext::unattended(Arc::new(RecordingObserver::default()), CancelToken::never());
        block_on(UndoOperation::new(self.mounts.clone(), self.history.clone()).execute(&ctx))?.result
    }
}

#[test]
fn copy_streams_between_providers_and_undo_deletes_the_copies() {
    let w = World::new();
    w.local.add_dir("/home/album");
    w.local.add_file_with("/home/album/a.jpg", b"0123456789abcdef");
    let request = TransferRequest { sources: vec![w.local.path("/home/album")], destination: w.remote.path("/bucket") };

    block_on(w.copy().execute(&request, &w.ctx(ConflictChoice::Stop))).result.unwrap();
    assert_eq!(w.remote.contents("/bucket/album/a.jpg").unwrap(), b"0123456789abcdef");

    w.undo().unwrap();
    assert!(!w.remote.exists("/bucket/album"), "no Trash remotely: the copy is deleted");
    assert!(w.local.exists("/home/album/a.jpg"), "the original is untouched");
    assert!(w.history.next_redo().is_none(), "a permanent delete can't be redone");
}

#[test]
fn move_across_providers_and_back() {
    let w = World::new();
    w.remote.add_file_with("/bucket/report.pdf", b"pdf!");
    let request =
        TransferRequest { sources: vec![w.remote.path("/bucket/report.pdf")], destination: w.local.path("/home") };

    block_on(MoveEntries::new(w.mounts.clone(), w.history.clone()).execute(&request, &w.ctx(ConflictChoice::Stop)))
        .result
        .unwrap();
    assert!(!w.remote.exists("/bucket/report.pdf"));
    assert_eq!(w.local.contents("/home/report.pdf").unwrap(), b"pdf!");

    w.undo().unwrap();
    assert_eq!(w.remote.contents("/bucket/report.pdf").unwrap(), b"pdf!");
    assert!(!w.local.exists("/home/report.pdf"));
}

#[test]
fn large_metered_transfers_ask_first_and_can_be_declined() {
    let w = World::new();
    w.local.add_file("/home/big.iso", 20);
    let costs = Arc::new(ScriptedCosts::new(CostDecision::Cancel));
    let ctx = JobContext { costs: costs.clone(), ..w.ctx(ConflictChoice::Stop) };
    let copy = w.copy().with_cost_policy(CostPolicy { large_transfer: ByteSize(10) });
    let request =
        TransferRequest { sources: vec![w.local.path("/home/big.iso")], destination: w.remote.path("/bucket") };

    let report = block_on(copy.execute(&request, &ctx));

    assert_eq!(report.result, Err(OperationError::Stopped));
    assert!(!w.remote.exists("/bucket/big.iso"));
    assert_eq!(costs.warnings()[0].reasons, vec![CostReason::LargeTransfer(ByteSize(20))]);
}

#[test]
fn cross_region_transfers_warn_even_when_small() {
    let w = World::new();
    let other = FakeVfs::remote("other", Some("us-east-1"));
    other.add_dir("/b");
    let registry = ProviderRegistry::default()
        .with_provider(w.remote.clone().into_factory())
        .and_then(|r| r.with_provider(other.clone().into_factory()))
        .unwrap();
    w.remote.add_file("/bucket/tiny", 1);
    let costs = Arc::new(ScriptedCosts::new(CostDecision::Proceed));
    let ctx = JobContext { costs: costs.clone(), ..w.ctx(ConflictChoice::Stop) };
    let request = TransferRequest { sources: vec![w.remote.path("/bucket/tiny")], destination: other.path("/b") };

    block_on(CopyEntries::new(Arc::new(registry), Arc::default()).execute(&request, &ctx)).result.unwrap();

    assert!(other.exists("/b/tiny"), "the user agreed");
    assert_eq!(
        costs.warnings()[0].reasons,
        vec![CostReason::CrossRegion { from: "eu-west-1".into(), to: "us-east-1".into() }]
    );
}

#[test]
fn replacing_where_there_is_no_trash_is_announced_and_permanent() {
    let w = World::new();
    w.local.add_file_with("/home/a.txt", b"new");
    w.remote.add_file_with("/bucket/a.txt", b"old");
    let resolver = Arc::new(ScriptedResolver::new(vec![ConflictResolution {
        choice: ConflictChoice::Replace,
        scope: ConflictScope::ThisItem,
    }]));
    let ctx = JobContext { resolver: resolver.clone(), ..w.ctx(ConflictChoice::Replace) };
    let request = TransferRequest { sources: vec![w.local.path("/home/a.txt")], destination: w.remote.path("/bucket") };

    block_on(w.copy().execute(&request, &ctx)).result.unwrap();

    assert_eq!(w.remote.contents("/bucket/a.txt").unwrap(), b"new");
    assert_eq!(resolver.times_asked(), 1);
}

#[test]
fn trashing_where_there_is_no_trash_is_refused() {
    let w = World::new();
    w.remote.add_file("/bucket/keep", 1);
    let trash = TrashEntries::new(w.mounts.clone(), w.history.clone());
    let report = block_on(trash.execute(&[w.remote.path("/bucket/keep")], &w.ctx(ConflictChoice::Stop)));
    assert!(matches!(report.result, Err(OperationError::Failed { error: VfsError::Unsupported(_), .. })));
    assert!(w.remote.exists("/bucket/keep"), "never silently deleted");
}

#[test]
fn remote_items_open_through_a_local_copy() {
    let w = World::new();
    w.remote.add_file("/bucket/slides.key", 1);
    let opener = Arc::new(RecordingOpener::default());
    block_on(OpenEntry::new(w.mounts.clone(), opener.clone()).execute(&w.remote.path("/bucket/slides.key"))).unwrap();
    assert_eq!(opener.opened().len(), 1);
}

#[test]
fn redo_after_undoing_a_local_copy_still_works() {
    let w = World::new();
    w.local.add_file("/home/a", 1);
    w.local.add_dir("/home/dst");
    let request = TransferRequest { sources: vec![w.local.path("/home/a")], destination: w.local.path("/home/dst") };
    block_on(w.copy().execute(&request, &w.ctx(ConflictChoice::Stop))).result.unwrap();
    w.undo().unwrap();
    let ctx = JobContext::unattended(Arc::new(RecordingObserver::default()), CancelToken::never());
    block_on(RedoOperation::new(w.mounts.clone(), w.history.clone()).execute(&ctx)).unwrap().result.unwrap();
    assert!(w.local.exists("/home/dst/a"));
}

struct Accounts {
    store: Arc<InMemoryAccounts>,
    keychain: Arc<InMemoryKeychain>,
    connector: Arc<FakeS3Connector>,
}

impl Accounts {
    fn new() -> Self {
        Self {
            store: Arc::default(),
            keychain: Arc::default(),
            connector: Arc::new(FakeS3Connector::with_buckets(&["photos", "backups"])),
        }
    }

    fn services(&self) -> AccountServices {
        AccountServices {
            store: self.store.clone(),
            credentials: self.keychain.clone(),
            connector: self.connector.clone(),
        }
    }
}

fn access_key_account() -> NewS3Account {
    NewS3Account {
        name: "MinIO".into(),
        endpoint: Some("http://localhost:9000".into()),
        region: Some("us-east-1".into()),
        auth: S3Auth::AccessKey { access_key_id: "minio".into() },
        addressing: Addressing::PathStyle,
    }
}

#[test]
fn adding_an_account_stores_the_secret_and_lists_buckets() {
    let a = Accounts::new();
    let (account, buckets) = block_on(
        AddS3Account::new(a.services())
            .execute(access_key_account(), Some(Secret::new(FakeS3Connector::VALID_SECRET.into()))),
    )
    .unwrap();
    assert_eq!(buckets.iter().map(|b| b.name.to_string()).collect::<Vec<_>>(), ["photos", "backups"]);
    assert_eq!(a.keychain.len(), 1);
    assert_eq!(a.store.all(), vec![account.clone()]);

    block_on(RemoveS3Account::new(a.services()).execute(account.id)).unwrap();
    assert!(a.store.all().is_empty() && a.keychain.is_empty() && a.connector.connected().is_empty());
}

#[test]
fn a_failing_account_leaves_nothing_behind() {
    let a = Accounts::new();
    let result =
        block_on(AddS3Account::new(a.services()).execute(access_key_account(), Some(Secret::new("wrong".into()))));
    assert_eq!(result, Err(AccountError::Connection(VfsError::PermissionDenied)));
    assert!(a.store.all().is_empty() && a.keychain.is_empty());

    let missing = block_on(AddS3Account::new(a.services()).execute(access_key_account(), None));
    assert_eq!(missing, Err(AccountError::MissingSecret));
}

#[test]
fn saved_accounts_reconnect_at_start_up() {
    let a = Accounts::new();
    block_on(
        AddS3Account::new(a.services())
            .execute(access_key_account(), Some(Secret::new(FakeS3Connector::VALID_SECRET.into()))),
    )
    .unwrap();
    let restarted = Arc::new(FakeS3Connector::with_buckets(&["photos"]));
    let services = AccountServices { connector: restarted.clone(), ..a.services() };
    let connected = block_on(ConnectSavedAccounts::new(services).execute()).unwrap();
    assert_eq!(connected.len(), 1);
    assert_eq!(connected[0].status, Ok(()));
    assert_eq!(restarted.connected().len(), 1);
}

#[test]
fn favorites_keep_order_and_ignore_duplicates() {
    let store = Arc::new(InMemoryFavorites::default());
    let fake = FakeVfs::new();
    for path in ["/a", "/b", "/a"] {
        block_on(AddFavorite::new(store.clone()).execute(fake.path(path))).unwrap();
    }
    block_on(RemoveFavorite::new(store.clone()).execute(fake.path("/a"))).unwrap();
    assert_eq!(block_on(ListFavorites::new(store).execute()).unwrap(), vec![fake.path("/b")]);
}

#[test]
fn buckets_added_by_name_show_even_when_listing_is_denied() {
    use application::use_cases::{AddBucket, ListBuckets};
    let a = Accounts::new();
    let connector = Arc::new(FakeS3Connector::denying_listing());
    let services = AccountServices { connector: connector.clone(), ..a.services() };
    let account = block_on(a.store.add_s3(access_key_account())).unwrap();
    block_on(connector.connect(&account, None)).unwrap();
    let list = ListBuckets::new(services.connector.clone(), a.store.clone());
    assert_eq!(block_on(list.execute(account.id)), Err(VfsError::PermissionDenied), "nothing to show yet");

    block_on(AddBucket::new(a.store).execute(account.id, domain::BucketName::new("team-data").unwrap())).unwrap();
    let buckets = block_on(list.execute(account.id)).unwrap();
    assert_eq!(buckets.iter().map(|b| b.name.to_string()).collect::<Vec<_>>(), ["team-data"]);
}
