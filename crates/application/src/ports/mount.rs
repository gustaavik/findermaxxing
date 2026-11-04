use std::fmt;
use std::sync::Arc;

use domain::{Capabilities, Capability, MountId, Scheme, VfsError};

use super::{
    DynCopier, DynFolderMaker, DynLauncher, DynLister, DynMaterializer, DynMover, DynReader, DynRemover, DynTrasher,
    DynWatcher, DynWriter, MetadataEditor, Presigner, VersionLister,
};

/// Declares the role slots of [`MountHandle`]: one optional `Arc<dyn Role>`
/// each, a builder method that also records the matching [`Capability`], and
/// an accessor that reports `Unsupported(capability)` when the role is absent.
macro_rules! mount_roles {
    ($($role:ident: $dyn_trait:ident => $capability:ident),+ $(,)?) => {
        /// The roles one mounted provider offers. Absent roles are `None`, so a
        /// read-only provider never has to stub out write methods.
        #[derive(Clone)]
        pub struct MountHandle {
            capabilities: Capabilities,
            region: Option<String>,
            $($role: Option<Arc<dyn $dyn_trait>>,)+
        }

        impl MountHandle {
            /// `semantics` are guarantees beyond role presence (`AtomicRename`, …).
            #[must_use]
            pub fn builder(semantics: Capabilities) -> MountHandleBuilder {
                MountHandleBuilder { handle: Self { capabilities: semantics, region: None, $($role: None,)+ } }
            }

            $(
                #[doc = concat!("# Errors\n`Unsupported(", stringify!($capability), ")` if the provider lacks this role.")]
                pub fn $role(&self) -> Result<Arc<dyn $dyn_trait>, VfsError> {
                    self.$role.clone().ok_or(VfsError::Unsupported(Capability::$capability))
                }
            )+
        }

        impl MountHandleBuilder {
            $(
                #[must_use]
                pub fn $role(mut self, $role: Arc<dyn $dyn_trait>) -> Self {
                    self.handle.$role = Some($role);
                    self.handle.capabilities = self.handle.capabilities.with(Capability::$capability);
                    self
                }
            )+
        }
    };
}

mount_roles! {
    lister: DynLister => List,
    watcher: DynWatcher => Watch,
    copier: DynCopier => Copy,
    mover: DynMover => Move,
    remover: DynRemover => Remove,
    folder_maker: DynFolderMaker => CreateFolder,
    trasher: DynTrasher => Trash,
    launcher: DynLauncher => Launch,
    materializer: DynMaterializer => LocalAccess,
    reader: DynReader => Read,
    writer: DynWriter => Write,
    metadata_editor: MetadataEditor => EditMetadata,
    presigner: Presigner => Presign,
    version_lister: VersionLister => ListVersions,
}

impl MountHandle {
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    /// For decorators (caching, offline access) that replace some roles and
    /// keep the rest.
    #[must_use]
    pub fn into_builder(self) -> MountHandleBuilder {
        MountHandleBuilder { handle: self }
    }

    /// The provider's region (for cross-region cost warnings), if it has one.
    #[must_use]
    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
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
    pub fn region(mut self, region: Option<String>) -> Self {
        self.handle.region = region;
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
