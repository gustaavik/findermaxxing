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

#[tokio::test(flavor = "multi_thread")]
async fn drive_accounts_persist_and_ids_are_not_reused() {
    use application::ports::{DriveAccountStore, NewDriveAccount};
    use domain::DriveProvider;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite3");
    let store = reopen(&path);
    let work = store
        .add_drive(NewDriveAccount {
            provider: DriveProvider::OneDrive,
            name: "Work".into(),
            email: "me@corp.test".into(),
        })
        .await
        .unwrap();
    store
        .add_drive(NewDriveAccount {
            provider: DriveProvider::GoogleDrive,
            name: "Alex".into(),
            email: "a@gmail.test".into(),
        })
        .await
        .unwrap();
    drop(store);
    let store = reopen(&path);
    let listed = store.list_drives().await.unwrap();
    assert_eq!(listed.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Alex", "Work"]);
    assert_eq!(listed[1], work);
    store.remove_drive(work.id).await.unwrap();
    let again = store
        .add_drive(NewDriveAccount { provider: DriveProvider::Dropbox, name: "Box".into(), email: String::new() })
        .await
        .unwrap();
    assert!(again.id.0 > work.id.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_semantic_index_survives_a_restart_and_filters_like_the_fake() {
    use application::ports::{IndexQuery, IndexedItem, IndexedPassage, ModelId, SemanticIndex};
    use domain::{ContentKind, FileCategory, Fingerprint};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite3");
    let model = ModelId("e5".into());
    let unit = |i: usize| -> Vec<f32> { (0..8).map(|j| if j == i { 1.0 } else { 0.0 }).collect() };
    let item = |name: &str, modified: i64| IndexedItem {
        location: location(&format!("file:///docs/{name}")),
        kind: ContentKind::Text,
        category: FileCategory::Text,
        fingerprint: Fingerprint { size: Some(ByteSize(10)), modified: Some(Timestamp::from_unix_millis(modified)) },
        model: model.clone(),
    };
    let store = reopen(&path);
    store
        .upsert(
            item("a.txt", 100),
            vec![
                IndexedPassage { snippet: "about cats".into(), vector: unit(0) },
                IndexedPassage { snippet: "about invoices".into(), vector: unit(1) },
            ],
        )
        .await
        .unwrap();
    store.upsert(item("b.txt", 200), vec![IndexedPassage { snippet: "dogs".into(), vector: unit(2) }]).await.unwrap();
    store.upsert(item("empty.bin", 300), Vec::new()).await.unwrap();
    drop(store);

    let store = reopen(&path);
    assert_eq!(store.count().await.unwrap(), 2, "items with nothing indexed don't count");
    assert_eq!(
        store.indexed(&location("file:///docs/empty.bin")).await.unwrap().map(|i| i.fingerprint.modified),
        Some(Some(Timestamp::from_unix_millis(300)))
    );
    let query = IndexQuery {
        vector: Some(unit(1)),
        kind: ContentKind::Text,
        model: model.clone(),
        limit: 10,
        modified: None,
        category: None,
        within: Vec::new(),
    };
    let hits = store.search(query.clone()).await.unwrap();
    assert_eq!(hits[0].item.location, location("file:///docs/a.txt"));
    assert_eq!(hits[0].snippet.as_deref(), Some("about invoices"), "the best passage is the snippet");
    assert!((hits[0].similarity - 1.0).abs() < 0.01);
    let other_model = store.search(IndexQuery { model: ModelId("other".into()), ..query.clone() }).await.unwrap();
    assert!(other_model.is_empty(), "spaces of different models never mix");
    let newest = store.search(IndexQuery { vector: None, ..query }).await.unwrap();
    assert_eq!(
        newest.iter().map(|h| h.item.location.path_string()).collect::<Vec<_>>(),
        ["/docs/b.txt", "/docs/a.txt"]
    );
    SemanticIndex::remove(&store, vec![location("file:///docs/a.txt")]).await.unwrap();
    drop(store);
    assert_eq!(reopen(&path).count().await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn cloud_model_settings_survive_a_restart_and_clear() {
    use application::ports::{CloudModelConfig, CloudModelSettings, CloudModelStore, CloudProvider};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite3");
    let settings = CloudModelSettings {
        config: CloudModelConfig {
            provider: CloudProvider::OpenAiCompatible,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.3".into(),
        },
        opted_in: vec![location("file:///Users/me/Downloads")],
    };
    assert_eq!(CloudModelStore::load(&reopen(&path)).await.unwrap(), None);
    CloudModelStore::save(&reopen(&path), settings.clone()).await.unwrap();
    assert_eq!(CloudModelStore::load(&reopen(&path)).await.unwrap(), Some(settings));
    CloudModelStore::clear(&reopen(&path)).await.unwrap();
    assert_eq!(CloudModelStore::load(&reopen(&path)).await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn ai_settings_survive_a_restart() {
    use application::ports::{AiSettings, AiSettingsStore};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite3");
    let settings = AiSettings {
        enabled: true,
        images: false,
        paused: true,
        scope: domain::IndexScope { include: vec![location("file:///docs")], exclude: Vec::new() },
    };
    assert_eq!(reopen(&path).load().await.unwrap(), None, "nothing chosen yet");
    reopen(&path).save(settings.clone()).await.unwrap();
    assert_eq!(reopen(&path).load().await.unwrap(), Some(settings));
}
