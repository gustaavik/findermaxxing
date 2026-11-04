#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use adapter_sqlite_store::SqliteStore;
use application::ports::{
    CacheIndex, CacheRecord, InFlightWrite, JournalEntry, JournaledKind, NewServerAccount, PinStore, ResumeState,
    ServerAccountStore, TransferJournal,
};
use domain::{
    ByteSize, EntryKind, EntryMetadata, FileEntry, PathSegment, ServerAuth, ServerProtocol, StoragePath, Timestamp,
    Transport, UnixMode, Visibility,
};

fn reopen(path: &std::path::Path) -> SqliteStore {
    SqliteStore::open(path, tokio::runtime::Handle::current()).unwrap()
}

fn location(text: &str) -> StoragePath {
    StoragePath::parse(text).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn server_accounts_persist_and_ids_are_not_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite3");
    let store = reopen(&path);
    let nas = NewServerAccount {
        protocol: ServerProtocol::Sftp,
        name: "NAS".into(),
        host: "nas.local".into(),
        port: Some(2222),
        transport: Transport::Tls,
        username: "me".into(),
        root: "/volume1".into(),
        auth: ServerAuth::KeyFile { path: "~/.ssh/id_ed25519".into() },
    };
    let dav = NewServerAccount {
        protocol: ServerProtocol::WebDav,
        name: "cloud".into(),
        transport: Transport::Plain,
        port: None,
        auth: ServerAuth::Password,
        ..nas.clone()
    };
    let first = store.add_server(nas.clone()).await.unwrap();
    store.add_server(dav).await.unwrap();
    drop(store);

    let store = reopen(&path);
    let listed = store.list_servers().await.unwrap();
    assert_eq!(listed.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["cloud", "NAS"]);
    assert_eq!(listed[1], first);
    assert_eq!(listed[0].transport, Transport::Plain);

    store.remove_server(first.id).await.unwrap();
    let again = store.add_server(nas).await.unwrap();
    assert!(again.id.0 > first.id.0, "ids name keychain items and are never reused");
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_records_listings_and_pins_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite3");
    let store = reopen(&path);
    let photo = location("sftp://3/trip/photo.jpg");
    let record = CacheRecord {
        location: photo.clone(),
        file: "sftp/3/trip/photo.jpg".into(),
        size: ByteSize(1234),
        remote_modified: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
        last_used: Timestamp::from_unix_millis(5),
    };
    store.record(record.clone()).await.unwrap();
    store.touch(&photo, Timestamp::from_unix_millis(9)).await.unwrap();

    let folder = location("sftp://3/trip");
    let odd_name = PathSegment::from_bytes(b"caf\xe9.txt").unwrap();
    let entries = vec![
        FileEntry::new(
            &folder,
            odd_name,
            EntryKind::File,
            EntryMetadata {
                size: Some(ByteSize(7)),
                modified: Some(Timestamp::from_unix_millis(42)),
                visibility: Visibility::Visible,
                permissions: Some(UnixMode::from_mode(0o640)),
            },
        ),
        FileEntry::new(
            &folder,
            PathSegment::new(".hidden").unwrap(),
            EntryKind::Directory,
            EntryMetadata { visibility: Visibility::Hidden, ..EntryMetadata::default() },
        ),
    ];
    store.save_listing(&folder, entries.clone()).await.unwrap();
    store.pin(folder.clone()).await.unwrap();
    store.pin(folder.clone()).await.unwrap();
    drop(store);

    let store = reopen(&path);
    let found = store.lookup(&photo).await.unwrap().unwrap();
    assert_eq!(found, CacheRecord { last_used: Timestamp::from_unix_millis(9), ..record });
    assert_eq!(store.listing(&folder).await.unwrap().unwrap(), entries, "names stay byte-exact");
    assert_eq!(store.listing(&location("sftp://3/other")).await.unwrap(), None);
    assert_eq!(store.pins().await.unwrap(), vec![folder.clone()]);

    store.remove(vec![photo.clone()]).await.unwrap();
    assert!(store.all().await.unwrap().is_empty());
    store.unpin(folder).await.unwrap();
    assert!(store.pins().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn unfinished_transfers_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite3");
    let store = reopen(&path);
    let entry = JournalEntry {
        kind: JournaledKind::Move,
        sources: vec![location("file:///Users/me/a.mov"), location("file:///Users/me/b")],
        destination: location("s3://2@media/"),
    };
    let write = InFlightWrite {
        source: location("file:///Users/me/a.mov"),
        target: location("s3://2@media/a.mov"),
        source_size: ByteSize(9_000_000_000),
        source_modified: Some(Timestamp::from_unix_millis(77)),
        state: ResumeState("upload-id-123".into()),
    };
    let unfinished = store.begin(entry.clone()).await.unwrap();
    store.checkpoint(unfinished, Some(write.clone())).await.unwrap();
    let finished = store.begin(entry.clone()).await.unwrap();
    store.finish(finished).await.unwrap();
    drop(store);

    let store = reopen(&path);
    let interrupted = store.interrupted().await.unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].id, unfinished);
    assert_eq!(interrupted[0].entry, entry);
    assert_eq!(interrupted[0].in_flight, Some(write));

    store.checkpoint(unfinished, None).await.unwrap();
    assert_eq!(store.interrupted().await.unwrap()[0].in_flight, None);
}
