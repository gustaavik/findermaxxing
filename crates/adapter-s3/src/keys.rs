//! `StoragePath` ↔ S3 key. Key segments are path segments; a "folder" is
//! the prefix `segments/…/`.

use domain::{PathError, PathSegment, StoragePath, VfsError};

pub(crate) const DELIMITER: char = '/';

/// The object key of an item (`photos/2024/a.jpg`). S3 keys are UTF-8.
pub(crate) fn key_of(path: &StoragePath) -> Result<String, VfsError> {
    let parts: Option<Vec<&str>> = path.segments().iter().map(PathSegment::as_str).collect();
    let parts = parts.ok_or_else(|| VfsError::InvalidPath(PathError::InvalidSegment(path.to_string())))?;
    if parts.is_empty() {
        return Err(VfsError::InvalidPath(PathError::Root));
    }
    Ok(parts.join("/"))
}

/// The listing prefix of a folder: `""` for the bucket root, else `key/`.
pub(crate) fn prefix_of(dir: &StoragePath) -> Result<String, VfsError> {
    if dir.is_root() {
        return Ok(String::new());
    }
    Ok(format!("{}{DELIMITER}", key_of(dir)?))
}

/// The item a key or common prefix names below `dir`, if it is a valid
/// segment (keys like `a//b` or `./x` can't be represented and are skipped).
pub(crate) fn child_of(dir: &StoragePath, prefix: &str, key: &str) -> Option<StoragePath> {
    let name = key.strip_prefix(prefix)?.trim_end_matches(DELIMITER);
    if name.is_empty() || name.contains(DELIMITER) {
        return None;
    }
    Some(dir.join(PathSegment::new(name).ok()?))
}

/// `copy_source` for CopyObject: `bucket/key`, URL-encoded except `/`.
pub(crate) fn copy_source(bucket: &str, key: &str) -> String {
    let mut out = format!("{bucket}/");
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str) -> StoragePath {
        StoragePath::parse(&format!("s3://1@bucket{p}")).unwrap()
    }

    #[test]
    fn keys_and_prefixes() {
        assert_eq!(key_of(&path("/a/b.txt")).unwrap(), "a/b.txt");
        assert_eq!(prefix_of(&path("/")).unwrap(), "");
        assert_eq!(prefix_of(&path("/a")).unwrap(), "a/");
        assert!(key_of(&path("/")).is_err());
    }

    #[test]
    fn children_skip_unrepresentable_keys() {
        let dir = path("/a");
        assert_eq!(child_of(&dir, "a/", "a/x.txt"), Some(path("/a/x.txt")));
        assert_eq!(child_of(&dir, "a/", "a/sub/"), Some(path("/a/sub")));
        assert_eq!(child_of(&dir, "a/", "a/"), None, "the folder's own marker");
        assert_eq!(child_of(&dir, "a/", "a/../x"), None);
    }

    #[test]
    fn copy_sources_are_url_encoded() {
        assert_eq!(copy_source("b", "dir/a b+c.txt"), "b/dir/a%20b%2Bc.txt");
    }
}
