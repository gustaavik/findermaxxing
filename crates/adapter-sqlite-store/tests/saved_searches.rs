#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use adapter_sqlite_store::{SqliteStore, raw_execute};
use application::ports::{NewSavedSearch, SavedSearchStore, StoreError};
use domain::{FileCategory, HiddenItems, ModifiedWithin, SearchQuery, SizeClass, StoragePath};

fn search(name: &str) -> NewSavedSearch {
    NewSavedSearch {
        name: name.to_owned(),
        scope: StoragePath::parse("file:///Users/me/Documents").unwrap(),
        query: SearchQuery {
            text: "invoice 2024".into(),
            category: Some(FileCategory::Pdf),
            modified: ModifiedWithin::Month,
            size: SizeClass::Medium,
            hidden: HiddenItems::Include,
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_searches_persist_across_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/metadata.sqlite3");
    let runtime = tokio::runtime::Handle::current();
    let store = SqliteStore::open(&path, runtime.clone()).unwrap();
    let b = store.save(search("b")).await.unwrap();
    let a = store.save(search("A")).await.unwrap();
    drop(store);

    let reopened = SqliteStore::open(&path, runtime).unwrap();
    assert_eq!(reopened.list().await.unwrap(), vec![a.clone(), b.clone()], "ordered by name, case-insensitively");
    reopened.delete(a.id).await.unwrap();
    assert_eq!(reopened.list().await.unwrap(), vec![b]);
}

#[tokio::test(flavor = "multi_thread")]
async fn unreadable_rows_are_reported_as_corrupt() {
    let store = SqliteStore::in_memory(tokio::runtime::Handle::current()).unwrap();
    raw_execute(&store, "INSERT INTO saved_searches (name, scope, query) VALUES ('x', 'file:///', '{\"text\":1}')")
        .unwrap();
    assert!(matches!(store.list().await, Err(StoreError::Corrupt(_))));
}
