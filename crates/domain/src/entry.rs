use crate::path::{PathSegment, StoragePath};
use crate::units::{ByteSize, Timestamp, UnixMode};

/// Upper bound on entries per [`EntryBatch`]. Small enough that the UI gets a
/// first paint quickly, large enough to amortise cross-thread hand-offs.
pub const LIST_BATCH_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymlinkTarget {
    File,
    Directory,
    Broken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Directory,
    Symlink(SymlinkTarget),
    Other,
}

impl EntryKind {
    /// True for directories and links to directories: things you can open as a folder.
    #[must_use]
    pub const fn is_container(self) -> bool {
        matches!(self, Self::Directory | Self::Symlink(SymlinkTarget::Directory))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    Visible,
    Hidden,
}

/// Optional metadata. Remote providers may not know every field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryMetadata {
    pub size: Option<ByteSize>,
    pub modified: Option<Timestamp>,
    pub visibility: Visibility,
    pub permissions: Option<UnixMode>,
}

impl Default for EntryMetadata {
    fn default() -> Self {
        Self { size: None, modified: None, visibility: Visibility::Visible, permissions: None }
    }
}

/// A single directory entry. Always has a parent, so it always has a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    path: StoragePath,
    name: PathSegment,
    kind: EntryKind,
    metadata: EntryMetadata,
}

impl FileEntry {
    #[must_use]
    pub fn new(parent: &StoragePath, name: PathSegment, kind: EntryKind, metadata: EntryMetadata) -> Self {
        let path = parent.join(name.clone());
        Self { path, name, kind, metadata }
    }

    #[must_use]
    pub fn path(&self) -> &StoragePath {
        &self.path
    }

    #[must_use]
    pub fn name(&self) -> &PathSegment {
        &self.name
    }

    #[must_use]
    pub fn kind(&self) -> EntryKind {
        self.kind
    }

    #[must_use]
    pub fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.metadata.visibility == Visibility::Hidden
    }
}

/// A non-empty-or-empty slice of a listing, at most [`LIST_BATCH_SIZE`] long.
pub type EntryBatch = Vec<FileEntry>;
