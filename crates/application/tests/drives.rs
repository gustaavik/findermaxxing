//! Cloud-drive accounts against fakes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use application::CancelToken;
use application::ports::{DriveConnector, SignInError};
use application::testing::{FakeDriveConnector, InMemoryDrives, InMemoryKeychain};
use application::use_cases::{AddDrive, ConnectDrive, DriveError, DriveServices, RemoveDrive};
use domain::{DriveProvider, VfsError};
use futures_executor::block_on;

fn services(
    connector: FakeDriveConnector,
) -> (DriveServices, Arc<InMemoryDrives>, Arc<InMemoryKeychain>, Arc<FakeDriveConnector>) {
    let (store, keychain, connector) =
        (Arc::new(InMemoryDrives::default()), Arc::new(InMemoryKeychain::default()), Arc::new(connector));
    let services = DriveServices {
        store: store.clone(),
        credentials: keychain.clone(),
        connectors: vec![connector.clone() as Arc<dyn DriveConnector>],
    };
    (services, store, keychain, connector)
}

#[test]
fn signing_in_saves_the_account_and_its_token_and_removal_forgets_both() {
    let (services, store, keychain, connector) = services(FakeDriveConnector::new(DriveProvider::Dropbox));
    let added =
        block_on(AddDrive::new(services.clone()).execute(DriveProvider::Dropbox, CancelToken::never())).unwrap();
    assert_eq!((added.name.as_str(), added.email.as_str()), ("Me", "me@example.com"));
    assert_eq!(keychain.len(), 1);
    assert_eq!(connector.connected(), vec![added.id]);

    connector.disconnect(added.id);
    block_on(ConnectDrive::new(services.clone()).execute(&added)).unwrap();
    assert_eq!(connector.connected(), vec![added.id], "reconnects with the stored token");

    block_on(RemoveDrive::new(services).execute(&added)).unwrap();
    assert!(store.all().is_empty() && keychain.is_empty() && connector.connected().is_empty());
}

#[test]
fn failed_or_cancelled_sign_ins_leave_nothing() {
    let denied = SignInError::Denied("access_denied".into());
    for (connector, expected) in [
        (FakeDriveConnector::failing_sign_in(DriveProvider::GoogleDrive, denied.clone()), DriveError::SignIn(denied)),
        (
            FakeDriveConnector::refusing_connect(DriveProvider::GoogleDrive),
            DriveError::SignIn(SignInError::Failed(VfsError::PermissionDenied)),
        ),
    ] {
        let (services, store, keychain, _) = services(connector);
        let result = block_on(AddDrive::new(services).execute(DriveProvider::GoogleDrive, CancelToken::never()));
        assert_eq!(result, Err(expected));
        assert!(store.all().is_empty() && keychain.is_empty());
    }
    let (services, store, _, _) = services(FakeDriveConnector::new(DriveProvider::OneDrive));
    let cancel = application::CancellationSource::new();
    cancel.cancel();
    let result = block_on(AddDrive::new(services.clone()).execute(DriveProvider::OneDrive, cancel.token()));
    assert_eq!(result, Err(DriveError::SignIn(SignInError::Cancelled)));
    let result = block_on(AddDrive::new(services).execute(DriveProvider::Dropbox, CancelToken::never()));
    assert_eq!(result, Err(DriveError::NoProvider(DriveProvider::Dropbox)));
    assert!(store.all().is_empty());
}
