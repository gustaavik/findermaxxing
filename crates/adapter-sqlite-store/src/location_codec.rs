//! JSON forms of locations and entries, built on the lossless location
//! encoding. Field names and kind strings are storage formats, decoupled
//! from Rust identifiers so refactors never break existing databases.

use application::ports::StoreError;
use domain::{
    ByteSize, EntryKind, EntryMetadata, FileEntry, StoragePath, SymlinkTarget, Timestamp, UnixMode, Visibility,
};
use serde::{Deserialize, Serialize};

pub(crate) fn corrupt(error: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(error.to_string())
}

pub(crate) fn decode_location(encoded: &str) -> Result<StoragePath, StoreError> {
    StoragePath::decode(encoded).map_err(corrupt)
}

pub(crate) fn encode_locations(locations: &[StoragePath]) -> Result<String, StoreError> {
    let encoded: Vec<String> = locations.iter().map(StoragePath::encode).collect();
    serde_json::to_string(&encoded).map_err(corrupt)
}

pub(crate) fn decode_locations(json: &str) -> Result<Vec<StoragePath>, StoreError> {
    let encoded: Vec<String> = serde_json::from_str(json).map_err(corrupt)?;
    encoded.iter().map(|location| decode_location(location)).collect()
}

#[derive(Serialize, Deserialize)]
struct StoredEntry {
    path: String,
    kind: String,
    size: Option<u64>,
    modified: Option<i64>,
    hidden: bool,
    mode: Option<u32>,
}

const FILE: &str = "file";
const DIRECTORY: &str = "dir";
const LINK_TO_FILE: &str = "link-file";
const LINK_TO_DIRECTORY: &str = "link-dir";
const BROKEN_LINK: &str = "link-broken";
const OTHER: &str = "other";

fn kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => FILE,
        EntryKind::Directory => DIRECTORY,
        EntryKind::Symlink(SymlinkTarget::File) => LINK_TO_FILE,
        EntryKind::Symlink(SymlinkTarget::Directory) => LINK_TO_DIRECTORY,
        EntryKind::Symlink(SymlinkTarget::Broken) => BROKEN_LINK,
        EntryKind::Other => OTHER,
    }
}

fn kind_of(name: &str) -> Result<EntryKind, StoreError> {
    Ok(match name {
        FILE => EntryKind::File,
        DIRECTORY => EntryKind::Directory,
        LINK_TO_FILE => EntryKind::Symlink(SymlinkTarget::File),
        LINK_TO_DIRECTORY => EntryKind::Symlink(SymlinkTarget::Directory),
        BROKEN_LINK => EntryKind::Symlink(SymlinkTarget::Broken),
        OTHER => EntryKind::Other,
        unknown => return Err(corrupt(format!("unknown entry kind {unknown:?}"))),
    })
}

pub(crate) fn encode_entries(entries: &[FileEntry]) -> Result<String, StoreError> {
    let stored: Vec<StoredEntry> = entries
        .iter()
        .map(|entry| {
            let metadata = entry.metadata();
            StoredEntry {
                path: entry.path().encode(),
                kind: kind_name(entry.kind()).to_owned(),
                size: metadata.size.map(|size| size.0),
                modified: metadata.modified.map(Timestamp::unix_millis),
                hidden: metadata.visibility == Visibility::Hidden,
                mode: metadata.permissions.map(UnixMode::bits),
            }
        })
        .collect();
    serde_json::to_string(&stored).map_err(corrupt)
}

pub(crate) fn decode_entries(json: &str) -> Result<Vec<FileEntry>, StoreError> {
    let stored: Vec<StoredEntry> = serde_json::from_str(json).map_err(corrupt)?;
    stored
        .into_iter()
        .map(|entry| {
            let path = decode_location(&entry.path)?;
            let (parent, name) = path.parent().zip(path.name().cloned()).ok_or_else(|| corrupt("a root entry"))?;
            let metadata = EntryMetadata {
                size: entry.size.map(ByteSize),
                modified: entry.modified.map(Timestamp::from_unix_millis),
                visibility: if entry.hidden { Visibility::Hidden } else { Visibility::Visible },
                permissions: entry.mode.map(UnixMode::from_mode),
            };
            Ok(FileEntry::new(&parent, name, kind_of(&entry.kind)?, metadata))
        })
        .collect()
}
