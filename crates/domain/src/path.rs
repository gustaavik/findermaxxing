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

/// One component of a path. Can never be empty, `.`, `..`, or contain `/` or NUL,
/// so a [`StoragePath`] built from segments can never escape its mount.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathSegment(String);

impl PathSegment {
    /// # Errors
    /// Returns [`PathError::InvalidSegment`] for empty, relative or separator-containing names.
    pub fn new(value: &str) -> Result<Self, PathError> {
        let is_reserved = matches!(value, "" | "." | "..");
        let has_forbidden_char = value.contains(SEGMENT_SEPARATOR) || value.contains('\0');
        if is_reserved || has_forbidden_char {
            Err(PathError::InvalidSegment(value.to_owned()))
        } else {
            Ok(Self(value.to_owned()))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Provider-independent absolute location: `<scheme>://<authority>/<seg>/<seg>`.
///
/// Segments are stored verbatim (no percent-encoding); the string form is a
/// display/location format, not a strict RFC 3986 URI.
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

    /// Parses `scheme://authority/a/b`. Empty segments (`//`) are ignored.
    ///
    /// # Errors
    /// Returns a [`PathError`] when the scheme, authority or any segment is invalid.
    pub fn parse(location: &str) -> Result<Self, PathError> {
        let (scheme, rest) =
            location.split_once(SCHEME_SEPARATOR).ok_or_else(|| PathError::MissingScheme(location.to_owned()))?;
        let (authority, path) = rest.split_once(SEGMENT_SEPARATOR).unwrap_or((rest, ""));
        let mount = MountId::new(Scheme::new(scheme)?, authority)?;
        let segments = path
            .split(SEGMENT_SEPARATOR)
            .filter(|s| !s.is_empty())
            .map(PathSegment::new)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { mount, segments })
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

    /// Path part only, always starting with `/` (the root is `/`).
    #[must_use]
    pub fn path_string(&self) -> String {
        let joined: Vec<&str> = self.segments.iter().map(PathSegment::as_str).collect();
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
}
