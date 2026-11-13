//! The license on this machine against fakes: activation, offline use,
//! renewal, a seat taken away on the server, and freeing the seat.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use application::ports::{CredentialKey, CredentialStore, LicenseError, LicenseStore};
use application::testing::{
    FakeMachineFiles, FixedMachine, InMemoryKeychain, InMemoryLicenseStore, ScriptedLicenseServer, fake_machine_file,
};
use application::use_cases::{ActivateLicense, CurrentLicense, DeactivateLicense, LicenseServices, RenewLicense};
use domain::{LicenseStatus, ProFeature, Timestamp};
use futures_executor::block_on;

const NOW: i64 = 1_793_793_600_000;
const DAY: i64 = 86_400_000;

fn at(days: i64) -> Timestamp {
    Timestamp::from_unix_millis(NOW + days * DAY)
}

struct World {
    server: Arc<ScriptedLicenseServer>,
    store: Arc<InMemoryLicenseStore>,
    keychain: Arc<InMemoryKeychain>,
    services: LicenseServices,
}

fn world(answers: Vec<Result<application::ports::MachineFile, LicenseError>>) -> World {
    let server = Arc::new(ScriptedLicenseServer::new(answers));
    let store = Arc::new(InMemoryLicenseStore::default());
    let keychain = Arc::new(InMemoryKeychain::default());
    let services = LicenseServices {
        server: server.clone(),
        reader: Arc::new(FakeMachineFiles),
        store: store.clone(),
        credentials: keychain.clone(),
        identity: Arc::new(FixedMachine("fp-1")),
    };
    World { server, store, keychain, services }
}

fn file(issued_days: i64, valid_days: i64) -> application::ports::MachineFile {
    fake_machine_file("Ada", "fp-1", None, NOW + issued_days * DAY, NOW + (issued_days + valid_days) * DAY)
}

fn current(w: &World, days: i64) -> LicenseStatus {
    block_on(CurrentLicense::new(w.services.clone()).execute(at(days))).unwrap()
}

#[test]
fn activation_unlocks_pro_and_works_offline_until_the_file_expires() {
    let w = world(vec![Ok(file(0, 30))]);
    assert_eq!(current(&w, 0), LicenseStatus::Free);
    let status = block_on(ActivateLicense::new(w.services.clone()).execute("  KEY-1 ", at(0))).unwrap();
    assert!(status.allows(ProFeature::Organize));
    assert_eq!(w.server.calls(), ["activate KEY-1 fp-1 Test Mac"]);
    assert!(block_on(w.keychain.load(&CredentialKey::license_key())).unwrap().is_some());
    assert!(current(&w, 20).is_pro(), "offline, within the file's life");
    assert!(matches!(current(&w, 31), LicenseStatus::NeedsConfirmation { .. }));
}

#[test]
fn failed_activations_keep_nothing() {
    let w = world(vec![Err(LicenseError::MachineLimit)]);
    let result = block_on(ActivateLicense::new(w.services.clone()).execute("KEY-1", at(0)));
    assert_eq!(result, Err(LicenseError::MachineLimit));
    assert!(block_on(w.keychain.load(&CredentialKey::license_key())).unwrap().is_none());
    assert!(block_on(w.store.load()).unwrap().is_none());
}

#[test]
fn old_files_are_renewed_and_offline_renewals_change_nothing() {
    let w = world(vec![Ok(file(0, 30)), Err(LicenseError::Network("offline".into())), Ok(file(25, 30))]);
    block_on(ActivateLicense::new(w.services.clone()).execute("KEY-1", at(0))).unwrap();
    let renew = RenewLicense::new(w.services.clone());
    assert!(block_on(renew.execute(at(5))).unwrap().is_pro());
    assert_eq!(w.server.calls().len(), 1, "not due yet");
    assert!(block_on(renew.execute(at(24))).unwrap().is_pro(), "offline: still the old file");
    assert!(block_on(renew.execute(at(25))).unwrap().is_pro());
    assert!(current(&w, 50).is_pro(), "the renewed file lasts to day 55");
}

#[test]
fn a_seat_removed_on_the_server_ends_pro_here() {
    let w = world(vec![Ok(file(0, 30)), Err(LicenseError::Ended)]);
    block_on(ActivateLicense::new(w.services.clone()).execute("KEY-1", at(0))).unwrap();
    let status = block_on(RenewLicense::new(w.services.clone()).execute(at(25))).unwrap();
    assert_eq!(status, LicenseStatus::Ended { licensee: "Ada".into() });
    assert_eq!(current(&w, 25), LicenseStatus::Free);
}

#[test]
fn deactivating_frees_the_seat_and_forgets_the_key_only_if_the_server_agreed() {
    let w = world(vec![Ok(file(0, 30)), Err(LicenseError::Network("offline".into())), Ok(file(0, 30))]);
    block_on(ActivateLicense::new(w.services.clone()).execute("KEY-1", at(0))).unwrap();
    let deactivate = DeactivateLicense::new(w.services.clone());
    assert!(block_on(deactivate.execute()).is_err());
    assert!(current(&w, 1).is_pro(), "kept, so the user can retry online");
    block_on(deactivate.execute()).unwrap();
    assert_eq!(current(&w, 1), LicenseStatus::Free);
    assert!(block_on(w.keychain.load(&CredentialKey::license_key())).unwrap().is_none());
}

#[test]
fn files_copied_from_another_machine_and_tampered_files_prove_nothing() {
    let w = world(Vec::new());
    block_on(w.store.save(fake_machine_file("Ada", "someone-else", None, NOW, NOW + 30 * DAY))).unwrap();
    assert_eq!(current(&w, 1), LicenseStatus::OtherMachine);
    block_on(w.store.save(application::ports::MachineFile("garbage".into()))).unwrap();
    assert_eq!(current(&w, 1), LicenseStatus::Free);
}
