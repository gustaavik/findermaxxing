//! Licensing fakes: machine files as plain text, a scripted server, a
//! fixed machine and an in-memory store.

use std::sync::{Mutex, PoisonError};

use domain::{Activation, Timestamp};
use futures::FutureExt;
use futures::future::{self, BoxFuture};

use crate::ports::{
    LicenseError, LicenseServer, LicenseStore, MachineFile, MachineFileReader, MachineIdentity, Secret, StoreError,
};

/// Fake machine files: `FAKE|licensee|fingerprint|license expiry ms or -|active|issued ms|file expiry ms`.
#[must_use]
pub fn fake_machine_file(
    licensee: &str,
    fingerprint: &str,
    license_expiry: Option<i64>,
    issued: i64,
    file_expiry: i64,
) -> MachineFile {
    let expiry = license_expiry.map_or_else(|| "-".to_owned(), |e| e.to_string());
    MachineFile(format!("FAKE|{licensee}|{fingerprint}|{expiry}|true|{issued}|{file_expiry}"))
}

#[derive(Debug, Default)]
pub struct FakeMachineFiles;

impl MachineFileReader for FakeMachineFiles {
    fn read(&self, file: &MachineFile) -> Result<Activation, LicenseError> {
        let bad = || LicenseError::BadFile("not a fake machine file".into());
        let parts: Vec<&str> = file.0.split('|').collect();
        let [tag, licensee, fingerprint, expiry, active, issued, file_expiry] = parts.as_slice() else {
            return Err(bad());
        };
        if *tag != "FAKE" {
            return Err(bad());
        }
        let millis = |s: &str| s.parse::<i64>().map(Timestamp::from_unix_millis).map_err(|_| bad());
        Ok(Activation {
            license_id: "fake-license".into(),
            licensee: (*licensee).to_owned(),
            fingerprint: (*fingerprint).to_owned(),
            license_expiry: if *expiry == "-" { None } else { Some(millis(expiry)?) },
            license_active: *active == "true",
            issued: millis(issued)?,
            file_expiry: Some(millis(file_expiry)?),
        })
    }
}

/// Answers from a script; remembers the calls.
#[derive(Debug)]
pub struct ScriptedLicenseServer {
    pub answers: Mutex<Vec<Result<MachineFile, LicenseError>>>,
    pub calls: Mutex<Vec<String>>,
}

impl ScriptedLicenseServer {
    #[must_use]
    pub fn new(answers: Vec<Result<MachineFile, LicenseError>>) -> Self {
        Self { answers: Mutex::new(answers), calls: Mutex::default() }
    }

    #[must_use]
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn next(&self, call: String) -> Result<MachineFile, LicenseError> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(call);
        let mut answers = self.answers.lock().unwrap_or_else(PoisonError::into_inner);
        if answers.is_empty() { Err(LicenseError::Network("no answer scripted".into())) } else { answers.remove(0) }
    }
}

impl LicenseServer for ScriptedLicenseServer {
    fn activate(
        &self,
        key: &Secret,
        fingerprint: &str,
        machine_name: &str,
    ) -> BoxFuture<'static, Result<MachineFile, LicenseError>> {
        future::ready(self.next(format!("activate {} {fingerprint} {machine_name}", key.expose()))).boxed()
    }

    fn renew(&self, key: &Secret, fingerprint: &str) -> BoxFuture<'static, Result<MachineFile, LicenseError>> {
        future::ready(self.next(format!("renew {} {fingerprint}", key.expose()))).boxed()
    }

    fn deactivate(&self, key: &Secret, fingerprint: &str) -> BoxFuture<'static, Result<(), LicenseError>> {
        future::ready(self.next(format!("deactivate {} {fingerprint}", key.expose())).map(drop)).boxed()
    }
}

#[derive(Debug)]
pub struct FixedMachine(pub &'static str);

impl MachineIdentity for FixedMachine {
    fn fingerprint(&self) -> String {
        self.0.to_owned()
    }

    fn name(&self) -> String {
        "Test Mac".to_owned()
    }
}

#[derive(Debug, Default)]
pub struct InMemoryLicenseStore {
    file: Mutex<Option<MachineFile>>,
}

impl LicenseStore for InMemoryLicenseStore {
    fn load(&self) -> BoxFuture<'static, Result<Option<MachineFile>, StoreError>> {
        future::ready(Ok(self.file.lock().unwrap_or_else(PoisonError::into_inner).clone())).boxed()
    }

    fn save(&self, file: MachineFile) -> BoxFuture<'static, Result<(), StoreError>> {
        *self.file.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
        future::ready(Ok(())).boxed()
    }

    fn clear(&self) -> BoxFuture<'static, Result<(), StoreError>> {
        *self.file.lock().unwrap_or_else(PoisonError::into_inner) = None;
        future::ready(Ok(())).boxed()
    }
}
