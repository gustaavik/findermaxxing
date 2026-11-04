#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use adapter_sqlite_store::SqliteStore;
use application::ports::{AccountStore, FavoritesStore, NewS3Account, SavedSearchStore};
use domain::{Addressing, S3Auth, StoragePath};

fn account(name: &str, auth: S3Auth) -> NewS3Account {
    NewS3Account {
        name: name.into(),
        endpoint: Some("http://127.0.0.1:9000".into()),
        region: None,
        auth,
        addressing: Addressing::PathStyle,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn accounts_persist_without_secrets_and_migrations_stack() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.sqlite3");
    let runtime = tokio::runtime::Handle::current();
    let store = SqliteStore::open(&path, runtime.clone()).unwrap();
    let minio = store.add_s3(account("MinIO", S3Auth::AccessKey { access_key_id: "AKIA".into() })).await.unwrap();
    let aws = store.add_s3(account("aws", S3Auth::Profile("work-sso".into()))).await.unwrap();
    store.set_skip_cost_warnings(aws.id, true).await.unwrap();
    store.add_bucket(minio.id, domain::BucketName::new("scoped").unwrap()).await.unwrap();
    store.add_bucket(minio.id, domain::BucketName::new("scoped").unwrap()).await.unwrap();
    drop(store);

    let reopened = SqliteStore::open(&path, runtime).unwrap();
    let listed = reopened.list_s3().await.unwrap();
    assert_eq!(listed.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["aws", "MinIO"]);
    assert!(listed[0].skip_cost_warnings);
    assert_eq!(listed[1].extra_buckets, vec![domain::BucketName::new("scoped").unwrap()], "added once");
    assert_eq!(listed[1].name, minio.name);
    assert!(SavedSearchStore::list(&reopened).await.unwrap().is_empty(), "migration 1's table still works");

    AccountStore::remove(&reopened, minio.id).await.unwrap();
    assert_eq!(reopened.list_s3().await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn favorites_keep_order_ignore_duplicates_and_survive_odd_names() {
    let store = SqliteStore::in_memory(tokio::runtime::Handle::current()).unwrap();
    let odd = StoragePath::parse("file:///data").unwrap().join(domain::PathSegment::from_bytes(b"caf\xe9").unwrap());
    let docs = StoragePath::parse("file:///Users/me/Documents").unwrap();
    for location in [docs.clone(), odd.clone(), docs.clone()] {
        FavoritesStore::add(&store, location).await.unwrap();
    }
    assert_eq!(FavoritesStore::list(&store).await.unwrap(), vec![docs.clone(), odd.clone()]);
    FavoritesStore::remove(&store, docs).await.unwrap();
    assert_eq!(FavoritesStore::list(&store).await.unwrap(), vec![odd]);
}

#[tokio::test(flavor = "multi_thread")]
async fn account_ids_are_never_reused() {
    let store = SqliteStore::in_memory(tokio::runtime::Handle::current()).unwrap();
    let first = store.add_s3(account("a", S3Auth::DefaultChain)).await.unwrap();
    AccountStore::remove(&store, first.id).await.unwrap();
    let second = store.add_s3(account("b", S3Auth::DefaultChain)).await.unwrap();
    assert_ne!(first.id, second.id, "a new account must not inherit the old one's keychain item");
}
