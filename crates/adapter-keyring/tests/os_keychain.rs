//! Round trip through the real OS keychain. Ignored by default because it
//! touches the user's keychain (and on Linux needs a running Secret Service).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use adapter_keyring::KeyringStore;
use application::ports::{CredentialKey, CredentialStore, Secret};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the real OS keychain"]
async fn secrets_round_trip_and_delete_cleanly() {
    let store = KeyringStore::new(tokio::runtime::Handle::current());
    let key =
        CredentialKey { service: "FinderMaxxing test".into(), account: format!("contract-{}", std::process::id()) };

    store.store(&key, &Secret::new("s3cr3t".into())).await.unwrap();
    assert_eq!(store.load(&key).await.unwrap(), Some(Secret::new("s3cr3t".into())));

    store.delete(&key).await.unwrap();
    assert_eq!(store.load(&key).await.unwrap(), None);
    store.delete(&key).await.unwrap();
}
