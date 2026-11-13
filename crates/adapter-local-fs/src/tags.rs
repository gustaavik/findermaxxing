//! `TagEditor` on native tags (ADR 0029): Finder tags on macOS, stored as a
//! binary property list (an array of `Name` or `Name\n<colour>`) in the
//! `com.apple.metadata:_kMDItemUserTags` attribute; `user.xdg.tags`
//! (comma-separated, as KDE's Dolphin and Baloo use) on Linux.

use std::io;
use std::path::Path;

use application::ports::TagEditor;
use domain::{Capability, StoragePath, Tag, VfsError};
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::error::from_io;
use crate::lister::LocalFs;
use crate::native_path::to_native;

#[cfg(target_os = "macos")]
const TAGS_ATTRIBUTE: &str = "com.apple.metadata:_kMDItemUserTags";
/// "No such attribute": its own errno on macOS, `ENODATA` on Linux.
#[cfg(target_os = "macos")]
const NO_ATTRIBUTE: i32 = libc::ENOATTR;
#[cfg(not(target_os = "macos"))]
const NO_ATTRIBUTE: i32 = libc::ENODATA;
#[cfg(not(target_os = "macos"))]
const TAGS_ATTRIBUTE: &str = "user.xdg.tags";

impl TagEditor for LocalFs {
    fn tags<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<Vec<Tag>, VfsError>> {
        async move {
            let native = to_native(path)?;
            self.blocking(move || read_tags(&native)).await
        }
        .boxed()
    }

    fn set_tags<'a>(&'a self, path: &'a StoragePath, tags: Vec<Tag>) -> BoxFuture<'a, Result<(), VfsError>> {
        async move {
            let native = to_native(path)?;
            self.blocking(move || write_tags(&native, &tags)).await
        }
        .boxed()
    }
}

fn attribute_error(error: &io::Error) -> VfsError {
    // ENOTSUP / EOPNOTSUPP: the file system has no extended attributes (FAT, some network mounts).
    if error.raw_os_error() == Some(libc::ENOTSUP) || error.raw_os_error() == Some(libc::EOPNOTSUPP) {
        VfsError::Unsupported(Capability::EditTags)
    } else {
        from_io(error)
    }
}

pub(crate) fn read_tags(path: &Path) -> Result<Vec<Tag>, VfsError> {
    std::fs::symlink_metadata(path).map_err(|e| from_io(&e))?;
    match xattr::get(path, TAGS_ATTRIBUTE).map_err(|e| attribute_error(&e))? {
        Some(bytes) => Ok(decode(&bytes).into_iter().filter_map(|name| Tag::new(&name).ok()).collect()),
        None => Ok(Vec::new()),
    }
}

pub(crate) fn write_tags(path: &Path, tags: &[Tag]) -> Result<(), VfsError> {
    std::fs::symlink_metadata(path).map_err(|e| from_io(&e))?;
    if tags.is_empty() {
        return match xattr::remove(path, TAGS_ATTRIBUTE) {
            Ok(()) => Ok(()),
            // Nothing to remove.
            Err(error) if error.raw_os_error() == Some(NO_ATTRIBUTE) => Ok(()),
            Err(error) => Err(attribute_error(&error)),
        };
    }
    let existing = xattr::get(path, TAGS_ATTRIBUTE).ok().flatten().map(|b| raw_entries(&b)).unwrap_or_default();
    let bytes = encode(tags, &existing)?;
    xattr::set(path, TAGS_ATTRIBUTE, &bytes).map_err(|e| attribute_error(&e))
}

/// Finder's entries as stored, `Name\n<colour>` included.
#[cfg(target_os = "macos")]
fn raw_entries(bytes: &[u8]) -> Vec<String> {
    match plist::Value::from_reader(io::Cursor::new(bytes)) {
        Ok(plist::Value::Array(items)) => items.into_iter().filter_map(|item| item.into_string()).collect(),
        _ => Vec::new(),
    }
}

#[cfg(not(target_os = "macos"))]
fn raw_entries(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).split(',').map(str::to_owned).collect()
}

/// Tag names, without Finder's colour suffix.
fn decode(bytes: &[u8]) -> Vec<String> {
    raw_entries(bytes)
        .into_iter()
        .map(|entry| entry.split('\n').next().unwrap_or_default().trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Keeps the colour Finder gave a tag that stays.
#[cfg(target_os = "macos")]
fn encode(tags: &[Tag], existing: &[String]) -> Result<Vec<u8>, VfsError> {
    let entries: Vec<String> = tags
        .iter()
        .map(|tag| {
            existing
                .iter()
                .find(|e| e.split('\n').next() == Some(tag.as_str()))
                .cloned()
                .unwrap_or_else(|| tag.as_str().to_owned())
        })
        .collect();
    binary_plist(entries)
}

#[cfg(target_os = "macos")]
fn binary_plist(entries: Vec<String>) -> Result<Vec<u8>, VfsError> {
    let mut bytes = Vec::new();
    plist::Value::Array(entries.into_iter().map(plist::Value::String).collect())
        .to_writer_binary(&mut bytes)
        .map_err(|e| VfsError::Io { message: e.to_string() })?;
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
fn encode(tags: &[Tag], _existing: &[String]) -> Result<Vec<u8>, VfsError> {
    Ok(tags.iter().map(Tag::as_str).collect::<Vec<_>>().join(",").into_bytes())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;

    #[test]
    fn tags_round_trip_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bill.pdf");
        std::fs::write(&file, b"x").unwrap();
        assert!(read_tags(&file).unwrap().is_empty());
        let tags = vec![Tag::new("Invoice").unwrap(), Tag::new("Home").unwrap()];
        write_tags(&file, &tags).unwrap();
        assert_eq!(read_tags(&file).unwrap(), tags);
        write_tags(&file, &[]).unwrap();
        assert!(read_tags(&file).unwrap().is_empty());
        write_tags(&file, &[]).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finder_colours_survive() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let finder = binary_plist(vec!["Red\n6".to_owned()]).unwrap();
        xattr::set(&file, TAGS_ATTRIBUTE, &finder).unwrap();
        write_tags(&file, &[Tag::new("Red").unwrap(), Tag::new("Work").unwrap()]).unwrap();
        let stored = raw_entries(&xattr::get(&file, TAGS_ATTRIBUTE).unwrap().unwrap());
        assert_eq!(stored, ["Red\n6", "Work"]);
    }
}
