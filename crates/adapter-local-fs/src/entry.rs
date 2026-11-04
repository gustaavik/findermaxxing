use std::fs::{self, Metadata};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use domain::{
    ByteSize, EntryKind, EntryMetadata, FileEntry, PathSegment, StoragePath, SymlinkTarget, Timestamp, UnixMode,
    Visibility,
};

const HIDDEN_PREFIX: char = '.';

/// Builds a [`FileEntry`] from `lstat` metadata of `native` (a child of `parent`).
pub(crate) fn build_entry(parent: &StoragePath, name: PathSegment, native: &Path, metadata: &Metadata) -> FileEntry {
    let kind = entry_kind(native, metadata);
    let entry_metadata = EntryMetadata {
        size: (kind == EntryKind::File).then_some(ByteSize(metadata.len())),
        modified: metadata.modified().ok().map(to_timestamp),
        visibility: visibility(&name, metadata),
        permissions: permissions(metadata),
    };
    FileEntry::new(parent, name, kind, entry_metadata)
}

fn entry_kind(native: &Path, metadata: &Metadata) -> EntryKind {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else if file_type.is_symlink() {
        EntryKind::Symlink(symlink_target(native))
    } else {
        EntryKind::Other
    }
}

fn symlink_target(native: &Path) -> SymlinkTarget {
    match fs::metadata(native) {
        Ok(target) if target.is_dir() => SymlinkTarget::Directory,
        Ok(_) => SymlinkTarget::File,
        Err(_) => SymlinkTarget::Broken,
    }
}

fn to_timestamp(time: SystemTime) -> Timestamp {
    let millis = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |m| -m),
    };
    Timestamp::from_unix_millis(millis)
}

fn visibility(name: &PathSegment, metadata: &Metadata) -> Visibility {
    if name.as_str().starts_with(HIDDEN_PREFIX) || has_hidden_flag(metadata) {
        Visibility::Hidden
    } else {
        Visibility::Visible
    }
}

#[cfg(target_os = "macos")]
fn has_hidden_flag(metadata: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    // `UF_HIDDEN` from <sys/stat.h>: set by `chflags hidden`, honoured by Finder.
    const UF_HIDDEN: u32 = 0x0000_8000;
    metadata.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(target_os = "macos"))]
fn has_hidden_flag(_metadata: &Metadata) -> bool {
    false
}

#[cfg(unix)]
fn permissions(metadata: &Metadata) -> Option<UnixMode> {
    use std::os::unix::fs::PermissionsExt;
    Some(UnixMode::from_mode(metadata.permissions().mode()))
}

#[cfg(not(unix))]
fn permissions(_metadata: &Metadata) -> Option<UnixMode> {
    None
}
