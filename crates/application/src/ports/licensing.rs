//! Ports of licensing (ADR 0030): the license server (Keygen), reading its
//! signed machine files, this machine's identity, and where the file is kept.

use domain::Activation;
use futures::future::BoxFuture;

use super::accounts::{CredentialError, Secret};
use super::saved_searches::StoreError;

/// A signed machine file, as the server issued it (PEM-style text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineFile(pub String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LicenseError {
    #[error("that license key isn't valid")]
    UnknownKey,
    #[error("this license has ended or is suspended")]
    Ended,
    #[error("this license is already in use on its maximum number of computers")]
    MachineLimit,
    #[error("the license server couldn't be reached: {0}")]
    Network(String),
    #[error("the license server answered with an error: {0}")]
    Service(String),
    #[error("the license file isn't valid: {0}")]
    BadFile(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Keychain(#[from] CredentialError),
}

pub trait LicenseServer: Send + Sync {
    /// Activates this machine (or finds its activation) and checks out a
    /// machine file.
    fn activate(
        &self,
        key: &Secret,
        fingerprint: &str,
        machine_name: &str,
    ) -> BoxFuture<'static, Result<MachineFile, LicenseError>>;
    /// A fresh machine file for an activated machine.
    fn renew(&self, key: &Secret, fingerprint: &str) -> BoxFuture<'static, Result<MachineFile, LicenseError>>;
    /// Frees this machine's seat.
    fn deactivate(&self, key: &Secret, fingerprint: &str) -> BoxFuture<'static, Result<(), LicenseError>>;
}

/// Verifies a machine file's signature and reads it.
pub trait MachineFileReader: Send + Sync {
    /// # Errors
    /// [`LicenseError::BadFile`] for a file that isn't signed by the server.
    fn read(&self, file: &MachineFile) -> Result<Activation, LicenseError>;
}

pub trait MachineIdentity: Send + Sync {
    /// Stable for this computer, and not reversible to its hardware id.
    fn fingerprint(&self) -> String;
    /// Shown in the license's list of computers.
    fn name(&self) -> String;
}

pub trait LicenseStore: Send + Sync {
    fn load(&self) -> BoxFuture<'static, Result<Option<MachineFile>, StoreError>>;
    fn save(&self, file: MachineFile) -> BoxFuture<'static, Result<(), StoreError>>;
    fn clear(&self) -> BoxFuture<'static, Result<(), StoreError>>;
}
