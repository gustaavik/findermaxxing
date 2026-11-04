//! `file://` URLs for the system clipboard (RFC 8089 / 3986). Only items on
//! the local mount have one; remote items get an in-app clipboard in M4.

use domain::{MountId, PathSegment, StoragePath};

const FILE_URL_PREFIX: &str = "file://";
const LOCALHOST: &str = "localhost";
const SEPARATOR: u8 = b'/';

/// `None` for items that are not on `local_mount`.
pub(crate) fn to_file_url(path: &StoragePath, local_mount: &MountId) -> Option<String> {
    if path.mount() != local_mount {
        return None;
    }
    let mut url = String::from(FILE_URL_PREFIX);
    if path.is_root() {
        url.push(char::from(SEPARATOR));
    }
    for segment in path.segments() {
        url.push(char::from(SEPARATOR));
        percent_encode_into(segment.as_bytes(), &mut url);
    }
    Some(url)
}

/// Parses `file:///a/b` (or `file://localhost/a/b`) into a local location.
pub(crate) fn from_file_url(url: &str, local_mount: &MountId) -> Option<StoragePath> {
    let rest = url.strip_prefix(FILE_URL_PREFIX)?;
    let path = rest.strip_prefix(LOCALHOST).unwrap_or(rest);
    if !path.starts_with(char::from(SEPARATOR)) {
        return None;
    }
    let bytes = percent_decode(path)?;
    bytes
        .split(|&b| b == SEPARATOR)
        .filter(|part| !part.is_empty())
        .try_fold(StoragePath::root(local_mount.clone()), |dir, part| {
            PathSegment::from_bytes(part).ok().map(|segment| dir.join(segment))
        })
}

fn percent_encode_into(bytes: &[u8], out: &mut String) {
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
}

fn percent_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(pair, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> MountId {
        StoragePath::parse("file:///").unwrap().mount().clone()
    }

    #[test]
    fn round_trips_awkward_names() {
        let dir = StoragePath::parse("file:///Users/me").unwrap();
        let path = dir.join(PathSegment::from_bytes(b"a #b?c%\xe9").unwrap());
        let url = to_file_url(&path, &local()).unwrap();
        assert_eq!(url, "file:///Users/me/a%20%23b%3Fc%25%E9");
        assert_eq!(from_file_url(&url, &local()).unwrap(), path);
    }

    #[test]
    fn accepts_localhost_and_rejects_other_schemes() {
        let parsed = from_file_url("file://localhost/tmp/x", &local()).unwrap();
        assert_eq!(parsed, StoragePath::parse("file:///tmp/x").unwrap());
        assert!(from_file_url("https://example.com/x", &local()).is_none());
        assert!(to_file_url(&StoragePath::parse("s3://b/k").unwrap(), &local()).is_none());
    }
}
