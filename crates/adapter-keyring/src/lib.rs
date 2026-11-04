//! Implements `CredentialStore` with the `keyring` crate, which picks the
//! platform store: Keychain Services on macOS, the Secret Service (GNOME
//! Keyring, KWallet) on Linux. Secrets never touch our own files.
//!
//! Keychain calls can block (Linux may unlock the keyring), so they run on
//! the runtime's blocking pool.

use application::ports::{CredentialError, CredentialKey, CredentialStore, Secret};
use futures::FutureExt;
use futures::future::BoxFuture;
use keyring::{Entry, Error};
use tokio::runtime::Handle;

#[derive(Debug, Clone)]
pub struct KeyringStore {
    runtime: Handle,
}

impl KeyringStore {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }

    fn run<T: Send + 'static>(
        &self,
        key: &CredentialKey,
        work: impl FnOnce(Entry) -> Result<T, Error> + Send + 'static,
    ) -> BoxFuture<'static, Result<T, CredentialError>> {
        let key = key.clone();
        let task = self.runtime.spawn_blocking(move || work(Entry::new(&key.service, &key.account)?));
        async move {
            match task.await {
                Ok(result) => result.map_err(|e| CredentialError(e.to_string())),
                Err(join) => Err(CredentialError(join.to_string())),
            }
        }
        .boxed()
    }
}

impl CredentialStore for KeyringStore {
    fn store(&self, key: &CredentialKey, secret: &Secret) -> BoxFuture<'static, Result<(), CredentialError>> {
        let secret = secret.clone();
        self.run(key, move |entry| entry.set_password(secret.expose()))
    }

    fn load(&self, key: &CredentialKey) -> BoxFuture<'static, Result<Option<Secret>, CredentialError>> {
        self.run(key, |entry| match entry.get_password() {
            Ok(password) => Ok(Some(Secret::new(password))),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(error),
        })
    }

    /// Deleting a secret that doesn't exist is not an error.
    fn delete(&self, key: &CredentialKey) -> BoxFuture<'static, Result<(), CredentialError>> {
        self.run(key, |entry| match entry.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(error) => Err(error),
        })
    }
}
