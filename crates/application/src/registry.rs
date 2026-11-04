use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use domain::{MountId, Scheme, VfsError};

use crate::ports::{MountHandle, MountResolver, ProviderFactory};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("a provider for scheme {0} is already registered")]
    DuplicateScheme(Scheme),
}

/// Maps URI schemes to provider factories. Built once by the composition root.
#[derive(Default)]
pub struct ProviderRegistry {
    factories: HashMap<Scheme, Arc<dyn ProviderFactory>>,
}

impl ProviderRegistry {
    /// # Errors
    /// [`RegistryError::DuplicateScheme`] if the scheme is taken.
    pub fn with_provider(mut self, factory: Arc<dyn ProviderFactory>) -> Result<Self, RegistryError> {
        let scheme = factory.scheme();
        if self.factories.contains_key(&scheme) {
            return Err(RegistryError::DuplicateScheme(scheme));
        }
        self.factories.insert(scheme, factory);
        Ok(self)
    }
}

impl MountResolver for ProviderRegistry {
    fn resolve(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        let factory =
            self.factories.get(mount.scheme()).ok_or_else(|| VfsError::UnknownScheme(mount.scheme().to_string()))?;
        factory.open(mount)
    }
}

impl fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRegistry").field("schemes", &self.factories.keys().collect::<Vec<_>>()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeVfs;
    use domain::StoragePath;

    #[test]
    fn resolves_registered_scheme_and_rejects_unknown() {
        let registry = ProviderRegistry::default().with_provider(FakeVfs::new().into_factory()).unwrap();
        let fake = StoragePath::parse("fake:///").unwrap();
        let unknown = StoragePath::parse("nope:///").unwrap();
        assert!(registry.resolve(fake.mount()).is_ok());
        assert_eq!(registry.resolve(unknown.mount()).unwrap_err(), VfsError::UnknownScheme("nope".into()));
    }

    #[test]
    fn rejects_duplicate_scheme() {
        let result = ProviderRegistry::default()
            .with_provider(FakeVfs::new().into_factory())
            .and_then(|r| r.with_provider(FakeVfs::new().into_factory()));
        assert!(matches!(result, Err(RegistryError::DuplicateScheme(_))));
    }
}
