use std::sync::Arc;

use application::ports::{MountHandle, ProviderFactory};
use domain::{Capabilities, Capability, MountId, PathError, Scheme, VfsError};

use crate::lister::LocalFs;
use crate::native_path::LOCAL_SCHEME;
use crate::trash::LocalTrash;

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
    trash: Arc<LocalTrash>,
}

impl LocalFsFactory {
    /// # Errors
    /// Never in practice; the scheme literal is validated like any other.
    pub fn new(fs: LocalFs, trash: LocalTrash) -> Result<Self, PathError> {
        Ok(Self { scheme: Scheme::new(LOCAL_SCHEME)?, fs: Arc::new(fs), trash: Arc::new(trash) })
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
        let fs = &self.fs;
        Ok(MountHandle::builder(LOCAL_SEMANTICS)
            .lister(fs.clone())
            .watcher(fs.clone())
            .copier(fs.clone())
            .mover(fs.clone())
            .remover(fs.clone())
            .folder_maker(fs.clone())
            .launcher(fs.clone())
            .materializer(fs.clone())
            .reader(fs.clone())
            .writer(fs.clone())
            .permission_editor(fs.clone())
            .secure_eraser(fs.clone())
            .trasher(self.trash.clone())
            .build())
    }
}
