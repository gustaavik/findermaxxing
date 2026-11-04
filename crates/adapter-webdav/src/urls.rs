//! Locations ↔ server URLs. Each path segment is percent-encoded byte for
//! byte, so any name the server can store round-trips.

use domain::{PathSegment, StoragePath};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// RFC 3986 unreserved characters stay as they are.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
const SEPARATOR: char = '/';

/// The account's root on the server: `https://host:port/base/path/`.
#[derive(Debug, Clone)]
pub(crate) struct Base {
    origin: String,
    /// Always starts and ends with `/`, percent-encoded.
    path: String,
}

impl Base {
    pub(crate) fn new(origin: String, root: &str) -> Self {
        let mut path = String::from(SEPARATOR);
        for part in root.split(SEPARATOR).filter(|part| !part.is_empty()) {
            path.push_str(&utf8_percent_encode(part, SEGMENT).to_string());
            path.push(SEPARATOR);
        }
        Self { origin, path }
    }

    /// `folder` adds the trailing slash collections should be addressed with.
    pub(crate) fn url(&self, location: &StoragePath, kind: Kind) -> String {
        let mut url = format!("{}{}", self.origin, self.path);
        let segments: Vec<String> = location.segments().iter().map(encode_segment).collect();
        url.push_str(&segments.join("/"));
        if kind == Kind::Folder && !segments.is_empty() {
            url.push(SEPARATOR);
        }
        url
    }

    /// The location an `href` from a multistatus response names, relative
    /// to `mount_root`. `None` for hrefs outside the account's root.
    pub(crate) fn location(&self, mount_root: &StoragePath, href: &str) -> Option<StoragePath> {
        let path = href.strip_prefix(&self.origin).unwrap_or(href);
        let path = if path.starts_with(SEPARATOR) { path.to_owned() } else { absolute_path_of_url(path)? };
        let decoded_base = percent_decode_str(&self.path).collect::<Vec<u8>>();
        let decoded = percent_decode_str(&path).collect::<Vec<u8>>();
        let relative = decoded.strip_prefix(decoded_base.as_slice()).or_else(|| {
            // The root itself, without its trailing slash.
            (decoded.as_slice() == &decoded_base[..decoded_base.len().saturating_sub(1)]).then_some(&[][..])
        })?;
        let mut location = mount_root.clone();
        for part in relative.split(|byte| *byte == b'/').filter(|part| !part.is_empty()) {
            location = location.join(PathSegment::from_bytes(part).ok()?);
        }
        Some(location)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Folder,
}

fn encode_segment(segment: &PathSegment) -> String {
    percent_encoding::percent_encode(segment.as_bytes(), SEGMENT).to_string()
}

/// `https://other.host/x/y` → `/x/y` (servers may answer with full URLs).
fn absolute_path_of_url(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://")?.1;
    let slash = after_scheme.find(SEPARATOR)?;
    Some(after_scheme[slash..].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> StoragePath {
        StoragePath::parse("dav://4/").unwrap()
    }

    #[test]
    fn urls_and_hrefs_round_trip_any_name() {
        let base = Base::new("https://cloud.example".into(), "remote.php/dav/files/me");
        let odd = root().join(PathSegment::from_bytes(b"caf\xe9 & #1").unwrap());
        let url = base.url(&odd, Kind::File);
        assert_eq!(url, "https://cloud.example/remote.php/dav/files/me/caf%E9%20%26%20%231");
        assert_eq!(base.location(&root(), &url), Some(odd.clone()));
        assert_eq!(base.location(&root(), "/remote.php/dav/files/me/caf%E9%20%26%20%231"), Some(odd));
        assert_eq!(base.location(&root(), "/remote.php/dav/files/me/"), Some(root()));
        assert_eq!(base.location(&root(), "/remote.php/dav/files/me"), Some(root()));
        assert_eq!(base.location(&root(), "/elsewhere/x"), None);
    }

    #[test]
    fn folders_get_a_trailing_slash() {
        let base = Base::new("http://nas:8080".into(), "");
        let folder = root().join(PathSegment::new("Photos").unwrap());
        assert_eq!(base.url(&folder, Kind::Folder), "http://nas:8080/Photos/");
        assert_eq!(base.url(&root(), Kind::Folder), "http://nas:8080/");
    }
}
