//! The Keygen client and machine-file verification against the Keygen double
//! in `mock-cloud`, which signs files with a test Ed25519 key.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use adapter_keygen::{KeygenAccount, KeygenMachineFiles, KeygenServer};
use application::ports::{LicenseError, LicenseServer, MachineFile, MachineFileReader, Secret};
use mock_cloud::MockCloud;
use mock_cloud::keygen::{MockLicense, public_key_hex};

async fn setup(max_machines: usize, status: &str) -> (MockCloud, KeygenServer, KeygenMachineFiles) {
    let mock = MockCloud::start().await;
    mock.state().keygen.licenses.push(MockLicense {
        id: "lic-1".into(),
        key: "KEY-1".into(),
        name: "Ada Lovelace".into(),
        status: status.into(),
        max_machines,
        machines: Vec::new(),
    });
    let account = KeygenAccount { base_url: format!("{}/keygen", mock.base()), account: "acme".into() };
    (mock, KeygenServer::new(account).unwrap(), KeygenMachineFiles::new(&public_key_hex()).unwrap())
}

fn key() -> Secret {
    Secret::new("KEY-1".into())
}

#[tokio::test(flavor = "multi_thread")]
async fn activation_yields_a_verified_file_for_this_machine() {
    let (mock, server, files) = setup(2, "ACTIVE").await;
    let file = server.activate(&key(), "fp-a", "Ada's Mac").await.unwrap();
    let activation = files.read(&file).unwrap();
    assert_eq!((activation.fingerprint.as_str(), activation.licensee.as_str()), ("fp-a", "Ada Lovelace"));
    assert!(activation.license_active && activation.license_expiry.is_none());
    assert!(activation.file_expiry.unwrap() > activation.issued);
    // Activating again (a lost answer, a reinstall) doesn't use another seat.
    server.activate(&key(), "fp-a", "Ada's Mac").await.unwrap();
    assert_eq!(mock.state().keygen.licenses[0].machines, ["fp-a"]);
    assert!(files.read(&server.renew(&key(), "fp-a").await.unwrap()).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn limits_unknown_keys_and_suspensions_are_told_apart() {
    let (_mock, server, _) = setup(1, "ACTIVE").await;
    server.activate(&key(), "fp-a", "one").await.unwrap();
    assert_eq!(server.activate(&key(), "fp-b", "two").await, Err(LicenseError::MachineLimit));
    assert_eq!(server.activate(&Secret::new("NOPE".into()), "fp-a", "x").await, Err(LicenseError::UnknownKey));
    let (_mock, suspended, _) = setup(1, "SUSPENDED").await;
    assert_eq!(suspended.activate(&key(), "fp-a", "x").await, Err(LicenseError::Ended));
}

#[tokio::test(flavor = "multi_thread")]
async fn deactivation_frees_the_seat() {
    let (mock, server, _) = setup(1, "ACTIVE").await;
    server.activate(&key(), "fp-a", "one").await.unwrap();
    server.deactivate(&key(), "fp-a").await.unwrap();
    assert!(mock.state().keygen.licenses[0].machines.is_empty());
    server.activate(&key(), "fp-b", "two").await.unwrap();
    assert_eq!(server.renew(&key(), "fp-a").await, Err(LicenseError::UnknownKey));
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_files_and_other_keys_are_rejected() {
    let (_mock, server, files) = setup(1, "ACTIVE").await;
    let file = server.activate(&key(), "fp-a", "one").await.unwrap();
    let body: String = file.0.lines().filter(|l| !l.starts_with("-----")).collect();
    let mut envelope: serde_json::Value = serde_json::from_slice(&base64_decode(&body)).unwrap();
    envelope["enc"] = serde_json::Value::String(envelope["enc"].as_str().unwrap().replacen('e', "f", 1));
    let forged = format!(
        "-----BEGIN MACHINE FILE-----\n{}\n-----END MACHINE FILE-----\n",
        base64_encode(envelope.to_string().as_bytes())
    );
    assert!(matches!(files.read(&MachineFile(forged)), Err(LicenseError::BadFile(_))));
    let other_key = KeygenMachineFiles::new(&"11".repeat(32));
    assert!(other_key.is_err() || other_key.unwrap().read(&file).is_err());
    assert!(KeygenMachineFiles::new("not hex").is_err());
}

fn base64_decode(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).unwrap()
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
