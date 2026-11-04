use std::fmt;
use std::sync::Arc;

use domain::{Capabilities, Capability, MountId, Scheme, VfsError};

use super::{DynLister, DynWatcher};

/// The roles one mounted provider offers. Absent roles are `None`, so a
/// read-only provider never has to stub out write methods; asking for a
/// missing role yields `VfsError::Unsupported`.
#[derive(Clone)]
pub struct MountHandle {
    capabilities: Capabilities,
    lister: Option<Arc<dyn DynLister>>,
    watcher: Option<Arc<dyn DynWatcher>>,
}

impl MountHandle {
    /// `semantics` are guarantees beyond role presence (`AtomicRename`, …).
    #[must_use]
    pub fn builder(semantics: Capabilities) -> MountHandleBuilder {
        MountHandleBuilder { handle: Self { capabilities: semantics, lister: None, watcher: None } }
    }

    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// # Errors
    /// `Unsupported(List)` if the provider cannot list.
    pub fn lister(&self) -> Result<Arc<dyn DynLister>, VfsError> {
        self.lister.clone().ok_or(VfsError::Unsupported(Capability::List))
    }

    /// # Errors
    /// `Unsupported(Watch)` if the provider cannot watch.
    pub fn watcher(&self) -> Result<Arc<dyn DynWatcher>, VfsError> {
        self.watcher.clone().ok_or(VfsError::Unsupported(Capability::Watch))
    }
}

impl fmt::Debug for MountHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MountHandle").field("capabilities", &self.capabilities).finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct MountHandleBuilder {
    handle: MountHandle,
}

impl MountHandleBuilder {
    #[must_use]
    pub fn lister(mut self, lister: Arc<dyn DynLister>) -> Self {
        self.handle.lister = Some(lister);
        self.handle.capabilities = self.handle.capabilities.with(Capability::List);
        self
    }

    #[must_use]
    pub fn watcher(mut self, watcher: Arc<dyn DynWatcher>) -> Self {
        self.handle.watcher = Some(watcher);
        self.handle.capabilities = self.handle.capabilities.with(Capability::Watch);
        self
    }

    #[must_use]
    pub fn build(self) -> MountHandle {
        self.handle
    }
}

/// Creates mount handles for one URI scheme. Registering a factory is the
/// only step needed to add a provider (Open/Closed).
pub trait ProviderFactory: Send + Sync {
    fn scheme(&self) -> Scheme;

    /// # Errors
    /// Provider-specific failures opening the mount (bad authority, auth, …).
    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError>;
}

/// Resolves a mount to its provider handle.
pub trait MountResolver: Send + Sync {
    /// # Errors
    /// `UnknownScheme` if no provider handles the mount's scheme.
    fn resolve(&self, mount: &MountId) -> Result<MountHandle, VfsError>;
}
