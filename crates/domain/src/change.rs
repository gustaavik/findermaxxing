use crate::entry::FileEntry;
use crate::path::StoragePath;

/// A change observed in a watched directory.
///
/// Renames are reported as `Removed(old)` + `Upserted(new)` so consumers need
/// only two operations. `Rescan` means events were lost (kernel queue overflow,
/// FSEvents `MustScanSubDirs`) and the listing must be reloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeEvent {
    Upserted(FileEntry),
    Removed(StoragePath),
    Rescan,
}

impl ChangeEvent {
    #[must_use]
    pub fn path(&self) -> Option<&StoragePath> {
        match self {
            Self::Upserted(entry) => Some(entry.path()),
            Self::Removed(path) => Some(path),
            Self::Rescan => None,
        }
    }
}
