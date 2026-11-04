use std::fs;
use std::path::{Path, PathBuf};

use domain::VfsError;
use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{NSFileManager, NSString, NSURL};

use crate::error::from_io;

/// Moves `path` to the Trash of its volume and returns where it ended up.
pub(super) fn trash(path: &Path) -> Result<PathBuf, VfsError> {
    fs::symlink_metadata(path).map_err(|e| from_io(&e))?;
    // APFS and HFS+ names are always valid UTF-8.
    let path = path.to_str().ok_or_else(|| VfsError::Io { message: "path is not valid UTF-8".to_owned() })?;
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let mut resulting: Option<Retained<NSURL>> = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
            .map_err(|error| VfsError::Io { message: error.localizedDescription().to_string() })?;
        let trashed = resulting
            .and_then(|url| url.path())
            .ok_or_else(|| VfsError::Io { message: "the Trash did not report where the item went".to_owned() })?;
        Ok(PathBuf::from(trashed.to_string()))
    })
}
