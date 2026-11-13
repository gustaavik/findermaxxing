//! The license on this machine (ADR 0030): what it allows now, activating
//! a key, renewing the signed machine file while online, and freeing the
//! seat. The key lives in the keychain; the machine file in the store.

use std::sync::Arc;

use domain::{LicenseStatus, Timestamp, license_status, should_renew};

use crate::ports::{
    CredentialKey, CredentialStore, LicenseError, LicenseServer, LicenseStore, MachineFile, MachineFileReader,
    MachineIdentity, Secret,
};

#[derive(Clone)]
pub struct LicenseServices {
    pub server: Arc<dyn LicenseServer>,
    pub reader: Arc<dyn MachineFileReader>,
    pub store: Arc<dyn LicenseStore>,
    pub credentials: Arc<dyn CredentialStore>,
    pub identity: Arc<dyn MachineIdentity>,
}

impl LicenseServices {
    async fn status(&self, now: Timestamp) -> Result<LicenseStatus, LicenseError> {
        let activation = match self.store.load().await? {
            // A file that doesn't verify proves nothing: free.
            Some(file) => self.reader.read(&file).ok(),
            None => None,
        };
        Ok(license_status(activation.as_ref(), &self.identity.fingerprint(), now))
    }

    async fn accept(&self, file: MachineFile, now: Timestamp) -> Result<LicenseStatus, LicenseError> {
        let activation = self.reader.read(&file)?;
        let status = license_status(Some(&activation), &self.identity.fingerprint(), now);
        self.store.save(file).await?;
        Ok(status)
    }
}

/// What the license allows right now (offline: from the stored file).
pub struct CurrentLicense {
    services: LicenseServices,
}

impl CurrentLicense {
    #[must_use]
    pub fn new(services: LicenseServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// The store failed.
    pub async fn execute(&self, now: Timestamp) -> Result<LicenseStatus, LicenseError> {
        self.services.status(now).await
    }
}

pub struct ActivateLicense {
    services: LicenseServices,
}

impl ActivateLicense {
    #[must_use]
    pub fn new(services: LicenseServices) -> Self {
        Self { services }
    }

    /// Activates this machine with `key`; the key is kept only if it worked.
    ///
    /// # Errors
    /// The key is unknown or used up, the server failed, or its file didn't verify.
    pub async fn execute(&self, key: &str, now: Timestamp) -> Result<LicenseStatus, LicenseError> {
        let s = &self.services;
        let key = Secret::new(key.trim().to_owned());
        if key.expose().is_empty() {
            return Err(LicenseError::UnknownKey);
        }
        let file = s.server.activate(&key, &s.identity.fingerprint(), &s.identity.name()).await?;
        let status = s.accept(file, now).await?;
        s.credentials.store(&CredentialKey::license_key(), &key).await?;
        Ok(status)
    }
}

/// Renews the machine file in the background when it gets old; failures
/// (offline) leave everything as it is.
pub struct RenewLicense {
    services: LicenseServices,
}

impl RenewLicense {
    #[must_use]
    pub fn new(services: LicenseServices) -> Self {
        Self { services }
    }

    /// The status after trying.
    ///
    /// # Errors
    /// Only the store's; the server's errors are kept quiet.
    pub async fn execute(&self, now: Timestamp) -> Result<LicenseStatus, LicenseError> {
        let s = &self.services;
        let Some(file) = s.store.load().await? else { return Ok(LicenseStatus::Free) };
        let due = match s.reader.read(&file) {
            Ok(activation) => should_renew(&activation, now) || activation.license_expiry.is_some_and(|e| e < now),
            Err(_) => true,
        };
        if due && let Some(key) = s.credentials.load(&CredentialKey::license_key()).await? {
            match s.server.renew(&key, &s.identity.fingerprint()).await {
                Ok(fresh) => return s.accept(fresh, now).await,
                // The server says the seat is gone: this machine is free now.
                Err(LicenseError::UnknownKey | LicenseError::Ended) => {
                    let ended = s.status(now).await;
                    s.store.clear().await?;
                    return ended.map(|status| match status {
                        LicenseStatus::Pro { licensee, .. } => LicenseStatus::Ended { licensee },
                        other => other,
                    });
                }
                Err(_) => {}
            }
        }
        s.status(now).await
    }
}

/// Frees this machine's seat, then forgets the license here.
pub struct DeactivateLicense {
    services: LicenseServices,
}

impl DeactivateLicense {
    #[must_use]
    pub fn new(services: LicenseServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// The server couldn't free the seat (nothing is forgotten then, so the
    /// user can try again online).
    pub async fn execute(&self) -> Result<(), LicenseError> {
        let s = &self.services;
        if let Some(key) = s.credentials.load(&CredentialKey::license_key()).await? {
            match s.server.deactivate(&key, &s.identity.fingerprint()).await {
                Ok(()) | Err(LicenseError::UnknownKey) => {}
                Err(error) => return Err(error),
            }
        }
        s.store.clear().await?;
        s.credentials.delete(&CredentialKey::license_key()).await?;
        Ok(())
    }
}

macro_rules! opaque_debug {
    ($($name:ident),+) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )+};
}

opaque_debug!(LicenseServices, CurrentLicense, ActivateLicense, RenewLicense, DeactivateLicense);
