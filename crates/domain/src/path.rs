use std::borrow::Cow;
use std::fmt;

use crate::error::PathError;

const SCHEME_SEPARATOR: &str = "://";
const SEGMENT_SEPARATOR: char = '/';
const SEGMENT_SEPARATOR_STR: &str = "/";

/// Lowercase URI scheme that selects a storage provider (`file`, `s3`, `sftp`, …).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Scheme(String);

impl Scheme {
    /// # Errors
    /// Returns [`PathError::InvalidScheme`] unless the value is a non-empty RFC 3986 scheme.
    pub fn new(value: &str) -> Result<Self, PathError> {
        let mut chars = value.chars();
        let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
        let rest_is_valid = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if starts_with_letter && rest_is_valid {
            Ok(Self(value.to_ascii_lowercase()))
        } else {
            Err(PathError::InvalidScheme(value.to_owned()))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies one mounted storage location: a provider scheme plus an authority
/// (empty for the local filesystem, a bucket name for S3, `user@host` for SFTP).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MountId {
    scheme: Scheme,
    authority: String,
}

impl MountId {
    /// # Errors
    /// Returns [`PathError::InvalidAuthority`] if the authority contains a path separator.
    pub fn new(scheme: Scheme, authority: &str) -> Result<Self, PathError> {
        if authority.contains(SEGMENT_SEPARATOR) {
            return Err(PathError::InvalidAuthority(authority.to_owned()));
        }
        Ok(Self { scheme, authority: authority.to_owned() })
    }

    #[must_use]
    pub fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }
}

/// One component of a path, stored as raw bytes so names that are not valid
/// UTF-8 (legal on Linux) survive every operation unchanged.
///
/// Can never be empty, `.`, `..`, or contain `/` or NUL, so a [`StoragePath`]
/// built from segments can never escape its mount.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathSegment(Box<[u8]>);

impl PathSegment {
    /// # Errors
    /// Returns [`PathError::InvalidSegment`] for empty, relative or separator-containing names.
    pub fn new(value: &str) -> Result<Self, PathError> {
        Self::from_bytes(value.as_bytes())
    }

    /// # Errors
    /// Returns [`PathError::InvalidSegment`] for empty, relative or separator-containing names.
    pub fn from_bytes(value: &[u8]) -> Result<Self, PathError> {
        let is_reserved = matches!(value, b"" | b"." | b"..");
        let has_forbidden_byte = value.iter().any(|&b| b == SEGMENT_SEPARATOR as u8 || b == 0);
        if is_reserved || has_forbidden_byte {
            Err(PathError::InvalidSegment(String::from_utf8_lossy(value).into_owned()))
        } else {
            Ok(Self(value.into()))
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The name as UTF-8, if it is valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Human-readable form; invalid bytes become U+FFFD.
    #[must_use]
    pub fn display_name(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    /// Splits `name.ext` into (`name`, `Some("ext")`). Leading-dot names such as
    /// `.bashrc` have no extension.
    #[must_use]
    pub fn stem_and_extension(&self) -> (&[u8], Option<&[u8]>) {
        match self.0.iter().rposition(|&b| b == b'.') {
            Some(dot) if dot > 0 && dot + 1 < self.0.len() => (&self.0[..dot], Some(&self.0[dot + 1..])),
            _ => (&self.0, None),
        }
    }

    fn encode_into(&self, out: &mut String) {
        for chunk in self.0.utf8_chunks() {
            for c in chunk.valid().chars() {
                if c == ESCAPE {
                    push_escaped(out, b'%');
                } else {
                    out.push(c);
                }
            }
            chunk.invalid().iter().for_each(|&byte| push_escaped(out, byte));
        }
    }

    fn decode(encoded: &str) -> Result<Self, PathError> {
        let bytes = encoded.as_bytes();
        let mut raw = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            match (bytes[index], hex_pair(bytes.get(index + 1..index + 3))) {
                (b'%', Some(byte)) => {
                    raw.push(byte);
                    index += 3;
                }
                (byte, _) => {
                    raw.push(byte);
                    index += 1;
                }
            }
        }
        Self::from_bytes(&raw)
    }
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name())
    }
}

/// Escape character of the lossless location encoding.
const ESCAPE: char = '%';

fn push_escaped(out: &mut String, byte: u8) {
    out.push(ESCAPE);
    out.push_str(&format!("{byte:02X}"));
}

fn hex_pair(pair: Option<&[u8]>) -> Option<u8> {
    let text = std::str::from_utf8(pair?).ok()?;
    u8::from_str_radix(text, 16).ok()
}

/// Provider-independent absolute location: `<scheme>://<authority>/<seg>/<seg>`.
///
/// Two string forms exist:
/// - [`StoragePath::parse`] / `Display`: what people type and read. Taken
///   literally; non-UTF-8 bytes display as U+FFFD.
/// - [`StoragePath::encode`] / [`StoragePath::decode`]: lossless, for handing a
///   location through string-only channels (Qt models, clipboards). Only `%`
///   and invalid UTF-8 bytes are percent-escaped.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StoragePath {
    mount: MountId,
    segments: Vec<PathSegment>,
}

impl StoragePath {
    #[must_use]
    pub fn root(mount: MountId) -> Self {
        Self { mount, segments: Vec::new() }
    }

    /// Parses `scheme://authority/a/b` literally. Empty segments (`//`) are ignored.
    ///
    /// # Errors
    /// Returns a [`PathError`] when the scheme, authority or any segment is invalid.
    pub fn parse(location: &str) -> Result<Self, PathError> {
        Self::parse_with(location, PathSegment::new)
    }

    /// Inverse of [`StoragePath::encode`].
    ///
    /// # Errors
    /// Returns a [`PathError`] when the scheme, authority or any segment is invalid.
    pub fn decode(encoded: &str) -> Result<Self, PathError> {
        Self::parse_with(encoded, PathSegment::decode)
    }

    fn parse_with(location: &str, segment: impl Fn(&str) -> Result<PathSegment, PathError>) -> Result<Self, PathError> {
        let (scheme, rest) =
            location.split_once(SCHEME_SEPARATOR).ok_or_else(|| PathError::MissingScheme(location.to_owned()))?;
        let (authority, path) = rest.split_once(SEGMENT_SEPARATOR).unwrap_or((rest, ""));
        let mount = MountId::new(Scheme::new(scheme)?, authority)?;
        let segments =
            path.split(SEGMENT_SEPARATOR).filter(|s| !s.is_empty()).map(segment).collect::<Result<Vec<_>, _>>()?;
        Ok(Self { mount, segments })
    }

    /// Lossless string form; see the type documentation.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = format!("{}{SCHEME_SEPARATOR}{}", self.mount.scheme, self.mount.authority);
        if self.segments.is_empty() {
            out.push(SEGMENT_SEPARATOR);
        }
        for segment in &self.segments {
            out.push(SEGMENT_SEPARATOR);
            segment.encode_into(&mut out);
        }
        out
    }

    #[must_use]
    pub fn mount(&self) -> &MountId {
        &self.mount
    }

    #[must_use]
    pub fn segments(&self) -> &[PathSegment] {
        &self.segments
    }

    #[must_use]
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    #[must_use]
    pub fn name(&self) -> Option<&PathSegment> {
        self.segments.last()
    }

    #[must_use]
    pub fn join(&self, segment: PathSegment) -> Self {
        let mut segments = self.segments.clone();
        segments.push(segment);
        Self { mount: self.mount.clone(), segments }
    }

    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let (_, parent_segments) = self.segments.split_last()?;
        Some(Self { mount: self.mount.clone(), segments: parent_segments.to_vec() })
    }

    #[must_use]
    pub fn is_child_of(&self, dir: &Self) -> bool {
        self.mount == dir.mount
            && self.segments.len() == dir.segments.len() + 1
            && self.segments.starts_with(&dir.segments)
    }

    /// True if `self` is `ancestor` or lies anywhere below it.
    #[must_use]
    pub fn is_within(&self, ancestor: &Self) -> bool {
        self.mount == ancestor.mount && self.segments.starts_with(&ancestor.segments)
    }

    /// Re-roots `self` from `old_base` onto `new_base` (`/a/b/c` from `/a` onto `/x` is `/x/b/c`).
    #[must_use]
    pub fn rebase(&self, old_base: &Self, new_base: &Self) -> Option<Self> {
        let tail = self.segments.strip_prefix(old_base.segments.as_slice())?;
        (self.mount == old_base.mount).then(|| {
            let mut segments = new_base.segments.clone();
            segments.extend_from_slice(tail);
            Self { mount: new_base.mount.clone(), segments }
        })
    }

    /// Human-readable path part, always starting with `/` (the root is `/`).
    #[must_use]
    pub fn path_string(&self) -> String {
        let joined: Vec<Cow<'_, str>> = self.segments.iter().map(PathSegment::display_name).collect();
        format!("{SEGMENT_SEPARATOR_STR}{}", joined.join(SEGMENT_SEPARATOR_STR))
    }
}

impl fmt::Display for StoragePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{SCHEME_SEPARATOR}{}{}", self.mount.scheme, self.mount.authority, self.path_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_rejects_traversal_and_separators() {
        for bad in ["", ".", "..", "a/b", "nul\0"] {
            assert!(PathSegment::new(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(PathSegment::new("..hidden").is_ok());
        assert!(PathSegment::new(".config").is_ok());
    }

    #[test]
    fn scheme_is_normalised_to_lowercase() {
        assert_eq!(Scheme::new("S3").unwrap().as_str(), "s3");
        assert!(Scheme::new("3s").is_err());
        assert!(Scheme::new("").is_err());
    }

    #[test]
    fn parse_and_display_round_trip() {
        let path = StoragePath::parse("file:///Users/me/My Documents").unwrap();
        assert_eq!(path.mount().scheme().as_str(), "file");
        assert_eq!(path.mount().authority(), "");
        assert_eq!(path.segments().len(), 3);
        assert_eq!(path.to_string(), "file:///Users/me/My Documents");
    }

    #[test]
    fn parse_with_authority() {
        let path = StoragePath::parse("s3://my-bucket/photos/2024").unwrap();
        assert_eq!(path.mount().authority(), "my-bucket");
        assert_eq!(path.path_string(), "/photos/2024");
    }

    #[test]
    fn parse_rejects_dot_dot() {
        assert!(StoragePath::parse("file:///a/../etc").is_err());
        assert!(StoragePath::parse("/no/scheme").is_err());
    }

    #[test]
    fn parent_join_and_child_relation() {
        let dir = StoragePath::parse("file:///a/b").unwrap();
        let child = dir.join(PathSegment::new("c").unwrap());
        assert!(child.is_child_of(&dir));
        assert!(!dir.is_child_of(&child));
        assert_eq!(child.parent().unwrap(), dir);
        assert_eq!(StoragePath::parse("file:///").unwrap().parent(), None);
    }

    #[test]
    fn non_utf8_names_round_trip_through_encode() {
        let dir = StoragePath::parse("file:///data").unwrap();
        let odd = dir.join(PathSegment::from_bytes(b"caf\xe9 100%.txt").unwrap());
        let encoded = odd.encode();
        assert_eq!(encoded, "file:///data/caf%E9 100%25.txt");
        assert_eq!(StoragePath::decode(&encoded).unwrap(), odd);
        assert_eq!(odd.name().unwrap().display_name(), "caf\u{fffd} 100%.txt");
        assert_eq!(StoragePath::decode("file:///").unwrap(), StoragePath::parse("file:///").unwrap());
    }

    #[test]
    fn parse_is_literal_about_percent_signs() {
        let path = StoragePath::parse("file:///tmp/50%25 off").unwrap();
        assert_eq!(path.name().unwrap().as_str(), Some("50%25 off"));
    }

    #[test]
    fn stem_and_extension() {
        let split = |n: &str| {
            let segment = PathSegment::new(n).unwrap();
            let (stem, ext) = segment.stem_and_extension();
            (String::from_utf8(stem.to_vec()).unwrap(), ext.map(|e| String::from_utf8(e.to_vec()).unwrap()))
        };
        assert_eq!(split("photo.jpeg"), ("photo".into(), Some("jpeg".into())));
        assert_eq!(split("archive.tar.gz"), ("archive.tar".into(), Some("gz".into())));
        assert_eq!(split(".bashrc"), (".bashrc".into(), None));
        assert_eq!(split("trailing."), ("trailing.".into(), None));
    }

    #[test]
    fn rebase_and_within() {
        let a = StoragePath::parse("file:///a").unwrap();
        let abc = StoragePath::parse("file:///a/b/c").unwrap();
        let x = StoragePath::parse("file:///x").unwrap();
        assert!(abc.is_within(&a) && a.is_within(&a) && !a.is_within(&abc));
        assert_eq!(abc.rebase(&a, &x).unwrap().to_string(), "file:///x/b/c");
        assert!(x.rebase(&a, &x).is_none());
    }
}
