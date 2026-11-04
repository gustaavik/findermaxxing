/// What a provider can do, and with which guarantees. Callers branch on these,
/// never on the concrete provider type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Capability {
    List,
    Read,
    Write,
    Watch,
    Copy,
    Move,
    Remove,
    CreateFolder,
    Trash,
    /// Open an item with the system's default application.
    Launch,
    /// Provide a local file for an item (identity for local disks, a cached
    /// download for remote providers), used by previews.
    LocalAccess,
    /// Requests and transfer are billed by the provider (S3 and friends).
    Metered,
    /// Shareable, expiring download links.
    Presign,
    /// Earlier versions of an item can be listed.
    ListVersions,
    /// Content type and custom metadata can be edited.
    EditMetadata,
    /// Rename is a single atomic operation (false on S3: copy + delete).
    AtomicRename,
    ServerSideCopy,
    PosixPermissions,
    Symlinks,
    RandomAccessRead,
    ResumableUpload,
    Versioning,
    /// Directories exist independently of their contents (false for S3 prefixes).
    RealDirectories,
}

/// Small immutable set of [`Capability`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities(u32);

impl Capabilities {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | Self::bit(capability))
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn contains(self, capability: Capability) -> bool {
        self.0 & Self::bit(capability) != 0
    }

    const fn bit(capability: Capability) -> u32 {
        1 << capability as u8
    }
}

impl FromIterator<Capability> for Capabilities {
    fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        iter.into_iter().fold(Self::empty(), Self::with)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_semantics() {
        let caps: Capabilities = [Capability::List, Capability::Watch].into_iter().collect();
        assert!(caps.contains(Capability::List));
        assert!(caps.contains(Capability::Watch));
        assert!(!caps.contains(Capability::AtomicRename));
        assert!(caps.union(Capabilities::empty().with(Capability::Trash)).contains(Capability::Trash));
    }
}
