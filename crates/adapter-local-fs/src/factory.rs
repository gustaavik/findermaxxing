use std::sync::Arc;

use application::ports::{MountHandle, ProviderFactory};
use domain::{Capabilities, Capability, MountId, PathError, Scheme, VfsError};

use crate::lister::LocalFs;
use crate::native_path::LOCAL_SCHEME;

const LOCAL_SEMANTICS: Capabilities = Capabilities::empty()
    .with(Capability::AtomicRename)
    .with(Capability::PosixPermissions)
    .with(Capability::Symlinks)
    .with(Capability::RandomAccessRead)
    .with(Capability::RealDirectories);

/// Registers `file://` with the provider registry.
#[derive(Debug, Clone)]
pub struct LocalFsFactory {
    scheme: Scheme,
    fs: Arc<LocalFs>,
}

impl LocalFsFactory {
    /// # Errors
    /// Never in practice; the scheme literal is validated like any other.
    pub fn new(fs: LocalFs) -> Result<Self, PathError> {
        Ok(Self { scheme: Scheme::new(LOCAL_SCHEME)?, fs: Arc::new(fs) })
    }
}

impl ProviderFactory for LocalFsFactory {
    fn scheme(&self) -> Scheme {
        self.scheme.clone()
    }

    fn open(&self, mount: &MountId) -> Result<MountHandle, VfsError> {
        if !mount.authority().is_empty() {
            return Err(VfsError::InvalidPath(PathError::InvalidAuthority(mount.authority().to_owned())));
        }
        Ok(MountHandle::builder(LOCAL_SEMANTICS).lister(self.fs.clone()).watcher(self.fs.clone()).build())
    }
}
