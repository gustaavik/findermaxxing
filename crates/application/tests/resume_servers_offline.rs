//! Resuming interrupted transfers, file-server accounts and offline pins,
//! against in-memory fakes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use application::operations::{AcceptCosts, JobContext, OperationHistory, RESUMABLE_SIZE};
use application::ports::ConnectError;
use application::ports::{
    InFlightWrite, JournalEntry, JournaledKind, MountResolver, NewServerAccount, PinStore, Secret, ServerConnector,
    TransferJournal,
};
use application::testing::{
    FakeServerConnector, FakeVfs, InMemoryJournal, InMemoryKeychain, InMemoryPins, InMemoryServers, RecordingObserver,
    ScriptedResolver,
};
use application::use_cases::{
    AddServer, ConnectServer, CopyEntries, DiscardTransfer, GetOfflineStatus, ListInterruptedTransfers,
    MakeAvailableOffline, OfflineStatus, RemoveOfflineCopy, RemoveServer, ResumeTransfer, ServerError, ServerServices,
    TransferRequest,
};
use application::{CancelToken, ProviderRegistry};
use domain::{HostKey, HostTrust, ServerAuth, ServerProtocol, Transport};
use futures_executor::block_on;

struct World {
    local: FakeVfs,
    remote: FakeVfs,
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    journal: Arc<InMemoryJournal>,
}

impl World {
    fn new() -> Self {
        let local = FakeVfs::new();
        let remote = FakeVfs::remote("remote", None);
        local.add_dir("/home");
        remote.add_dir("/bucket");
        let registry = ProviderRegistry::default()
            .with_provider(local.clone().into_factory())
            .and_then(|r| r.with_provider(remote.clone().into_factory()))
            .unwrap();
        Self { local, remote, mounts: Arc::new(registry), history: Arc::default(), journal: Arc::default() }
    }

    /// Any conflict question fails the test by stopping the job.
    fn ctx(&self) -> JobContext {
        JobContext {
            observer: Arc::new(RecordingObserver::default()),
            resolver: Arc::new(ScriptedResolver::new(Vec::new())),
            costs: Arc::new(AcceptCosts),
            cancel: CancelToken::never(),
        }
    }

    fn resume(&self) -> ResumeTransfer {
        ResumeTransfer::new(self.mounts.clone(), self.history.clone(), self.journal.clone())
    }

    fn interrupted(&self) -> usize {
        block_on(ListInterruptedTransfers::new(self.journal.clone()).execute()).unwrap().len()
    }
}

fn large_content() -> Vec<u8> {
    let size = usize::try_from(RESUMABLE_SIZE.0).unwrap() + 1000;
    (0..size).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

#[test]
fn large_writes_are_journaled_and_finished_jobs_leave_nothing_to_resume() {
    let w = World::new();
    w.local.add_file_with("/home/video.mov", &large_content());
    w.local.add_file_with("/home/small.txt", b"tiny");
    let copy = CopyEntries::new(w.mounts.clone(), w.history.clone()).with_journal(w.journal.clone());
    let request = TransferRequest {
        sources: vec![w.local.path("/home/video.mov"), w.local.path("/home/small.txt")],
        destination: w.remote.path("/bucket"),
    };

    block_on(copy.execute(&request, &w.ctx())).result.unwrap();

    let checkpoints = w.journal.checkpoints();
    assert_eq!(checkpoints.len(), 2, "the large file is journaled once and cleared once: {checkpoints:?}");
    assert_eq!(checkpoints[0].as_ref().unwrap().target, w.remote.path("/bucket/video.mov"));
    assert!(checkpoints[1].is_none());
    assert_eq!(w.interrupted(), 0);
    assert_eq!(w.remote.partial_count(), 0);
}

#[test]
fn an_interrupted_copy_continues_its_partial_file_and_keeps_finished_ones() {
    let w = World::new();
    let content = large_content();
    w.local.add_dir("/home/shoot");
    w.local.add_file_with("/home/shoot/a.raw", b"first");
    w.local.add_file_with("/home/shoot/b.raw", &content);
    // The earlier run created the folder, finished a.raw and got 5 MB into b.raw.
    w.remote.add_dir("/bucket/shoot");
    w.remote.add_file_with("/bucket/shoot/a.raw", b"first");
    let done = 5_000_000;
    let state = w.remote.seed_partial("/bucket/shoot/b.raw", &content[..done]);
    let interrupted = w.journal.seed(
        JournalEntry {
            kind: JournaledKind::Copy,
            sources: vec![w.local.path("/home/shoot")],
            destination: w.remote.path("/bucket"),
        },
        Some(InFlightWrite {
            source: w.local.path("/home/shoot/b.raw"),
            target: w.remote.path("/bucket/shoot/b.raw"),
            source_size: domain::ByteSize(content.len() as u64),
            source_modified: None,
            state,
        }),
    );

    block_on(w.resume().execute(&interrupted, &w.ctx())).result.unwrap();

    assert_eq!(w.remote.contents("/bucket/shoot/b.raw").unwrap(), content);
    assert_eq!(w.remote.contents("/bucket/shoot/a.raw").unwrap(), b"first");
    assert_eq!(w.local.read_offsets(), vec![done as u64], "only the missing part was read");
    assert_eq!(w.interrupted(), 0);
    assert_eq!(w.remote.partial_count(), 0);
}

#[test]
fn a_changed_source_restarts_its_file_and_discards_the_partial() {
    let w = World::new();
    let content = large_content();
    w.local.add_file_with("/home/b.raw", &content);
    let state = w.remote.seed_partial("/bucket/b.raw", b"stale bytes");
    let interrupted = w.journal.seed(
        JournalEntry {
            kind: JournaledKind::Copy,
            sources: vec![w.local.path("/home/b.raw")],
            destination: w.remote.path("/bucket"),
        },
        Some(InFlightWrite {
            source: w.local.path("/home/b.raw"),
            target: w.remote.path("/bucket/b.raw"),
            source_size: domain::ByteSize(3),
            source_modified: None,
            state,
        }),
    );

    block_on(w.resume().execute(&interrupted, &w.ctx())).result.unwrap();

    assert_eq!(w.remote.contents("/bucket/b.raw").unwrap(), content);
    assert!(w.local.read_offsets().is_empty(), "read from the start");
    assert_eq!(w.remote.partial_count(), 0);
}

#[test]
fn resuming_a_move_skips_what_was_already_moved() {
    let w = World::new();
    w.local.add_file_with("/home/left.txt", b"left");
    // moved.txt reached the remote and left home before the interruption.
    w.remote.add_file_with("/bucket/moved.txt", b"moved");
    let interrupted = w.journal.seed(
        JournalEntry {
            kind: JournaledKind::Move,
            sources: vec![w.local.path("/home/moved.txt"), w.local.path("/home/left.txt")],
            destination: w.remote.path("/bucket"),
        },
        None,
    );

    block_on(w.resume().execute(&interrupted, &w.ctx())).result.unwrap();

    assert_eq!(w.remote.contents("/bucket/left.txt").unwrap(), b"left");
    assert!(!w.local.exists("/home/left.txt"));
    assert_eq!(w.remote.contents("/bucket/moved.txt").unwrap(), b"moved");
}

#[test]
fn discarding_forgets_the_job_and_deletes_the_partial() {
    let w = World::new();
    let state = w.remote.seed_partial("/bucket/b.raw", b"partial");
    let interrupted = w.journal.seed(
        JournalEntry {
            kind: JournaledKind::Copy,
            sources: vec![w.local.path("/home/b.raw")],
            destination: w.remote.path("/bucket"),
        },
        Some(InFlightWrite {
            source: w.local.path("/home/b.raw"),
            target: w.remote.path("/bucket/b.raw"),
            source_size: domain::ByteSize(7),
            source_modified: None,
            state,
        }),
    );

    block_on(DiscardTransfer::new(w.mounts.clone(), w.journal.clone()).execute(&interrupted)).unwrap();

    assert_eq!(w.interrupted(), 0);
    assert_eq!(w.remote.partial_count(), 0);
    assert!(!w.remote.exists("/bucket/b.raw"));
}

fn sftp_account() -> NewServerAccount {
    NewServerAccount {
        protocol: ServerProtocol::Sftp,
        name: "NAS".into(),
        host: "nas.local".into(),
        port: None,
        transport: Transport::Tls,
        username: "me".into(),
        root: String::new(),
        auth: ServerAuth::Password,
    }
}

fn host_key() -> HostKey {
    HostKey { algorithm: "ssh-ed25519".into(), fingerprint: "SHA256:abc".into() }
}

struct Servers {
    store: Arc<InMemoryServers>,
    keychain: Arc<InMemoryKeychain>,
    connector: Arc<FakeServerConnector>,
    services: ServerServices,
}

fn servers(connector: FakeServerConnector) -> Servers {
    let store = Arc::new(InMemoryServers::default());
    let keychain = Arc::new(InMemoryKeychain::default());
    let connector = Arc::new(connector);
    let services = ServerServices {
        store: store.clone(),
        credentials: keychain.clone(),
        connectors: vec![connector.clone() as Arc<dyn ServerConnector>],
    };
    Servers { store, keychain, connector, services }
}

fn secret() -> Option<Secret> {
    Some(Secret::new(FakeServerConnector::VALID_SECRET.into()))
}

#[test]
fn an_unknown_host_key_must_be_accepted_and_nothing_is_saved_until_then() {
    let s = servers(FakeServerConnector::with_host_key(ServerProtocol::Sftp, host_key()));
    let add = AddServer::new(s.services.clone());

    let first = block_on(add.execute(sftp_account(), secret(), HostTrust::KnownOnly));
    assert_eq!(first, Err(ServerError::Connect(ConnectError::UnknownHost(host_key()))));
    assert!(s.store.all().is_empty());
    assert!(s.keychain.is_empty());

    let added = block_on(add.execute(sftp_account(), secret(), HostTrust::Accept(host_key()))).unwrap();
    assert_eq!(s.connector.connected(), vec![added.id]);
    assert_eq!(s.keychain.len(), 1);

    // Later connections trust the remembered key without asking.
    s.connector.disconnect(added.id);
    block_on(ConnectServer::new(s.services).execute(&added)).unwrap();
}

#[test]
fn wrong_passwords_roll_back_and_removal_cleans_up() {
    let s = servers(FakeServerConnector::new(ServerProtocol::Sftp));
    let add = AddServer::new(s.services.clone());
    let wrong = block_on(add.execute(sftp_account(), Some(Secret::new("nope".into())), HostTrust::KnownOnly));
    assert!(matches!(wrong, Err(ServerError::Connect(ConnectError::Failed(domain::VfsError::PermissionDenied)))));
    assert!(s.store.all().is_empty() && s.keychain.is_empty());

    let added = block_on(add.execute(sftp_account(), secret(), HostTrust::KnownOnly)).unwrap();
    block_on(RemoveServer::new(s.services.clone()).execute(&added)).unwrap();
    assert!(s.store.all().is_empty() && s.keychain.is_empty() && s.connector.connected().is_empty());
}

#[test]
fn protocols_without_a_provider_are_refused() {
    let s = servers(FakeServerConnector::new(ServerProtocol::Sftp));
    let smb = NewServerAccount { protocol: ServerProtocol::Smb, ..sftp_account() };
    let result = block_on(AddServer::new(s.services).execute(smb, secret(), HostTrust::KnownOnly));
    assert_eq!(result, Err(ServerError::NoProvider(ServerProtocol::Smb)));
}

#[test]
fn pinning_downloads_everything_and_reports_status() {
    let w = World::new();
    w.remote.add_dir("/bucket/trip");
    w.remote.add_file_with("/bucket/trip/a.jpg", b"aaaa");
    w.remote.add_file_with("/bucket/trip/b.jpg", b"bbbbbb");
    let pins = Arc::new(InMemoryPins::default());
    let observer = Arc::new(RecordingObserver::default());
    let ctx = JobContext { observer: observer.clone(), ..w.ctx() };

    block_on(MakeAvailableOffline::new(w.mounts.clone(), pins.clone()).execute(&w.remote.path("/bucket/trip"), &ctx))
        .unwrap();

    let progress = observer.last().unwrap();
    assert_eq!(progress.bytes_done, domain::ByteSize(10));
    assert_eq!(progress.items_done, 3);
    let status = GetOfflineStatus::new(pins.clone());
    assert_eq!(block_on(status.execute(&w.remote.path("/bucket/trip"))).unwrap(), OfflineStatus::Pinned);
    assert_eq!(
        block_on(status.execute(&w.remote.path("/bucket/trip/a.jpg"))).unwrap(),
        OfflineStatus::InPinnedFolder(w.remote.path("/bucket/trip"))
    );

    block_on(RemoveOfflineCopy::new(pins.clone()).execute(w.remote.path("/bucket/trip"))).unwrap();
    assert!(block_on(pins.pins()).unwrap().is_empty());
}

#[test]
fn the_journal_keeps_jobs_whose_run_was_dropped() {
    let w = World::new();
    let journal: Arc<dyn TransferJournal> = w.journal.clone();
    let id = block_on(journal.begin(JournalEntry {
        kind: JournaledKind::Copy,
        sources: vec![w.local.path("/home")],
        destination: w.remote.path("/bucket"),
    }))
    .unwrap();
    // No `finish`: the process "quit" here.
    let listed = block_on(ListInterruptedTransfers::new(w.journal).execute()).unwrap();
    assert_eq!(listed.iter().map(|t| t.id).collect::<Vec<_>>(), vec![id]);
}
