use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use domain::{MountId, PathError, PathSegment, Scheme, StoragePath, VfsError};

pub const LOCAL_SCHEME: &str = "file";
const NATIVE_ROOT: &str = "/";

/// Maps a `file:///a/b` location to `/a/b`.
pub(crate) fn to_native(path: &StoragePath) -> Result<PathBuf, VfsError> {
    if path.mount().scheme().as_str() != LOCAL_SCHEME || !path.mount().authority().is_empty() {
        return Err(VfsError::InvalidPath(PathError::InvalidAuthority(path.mount().authority().to_owned())));
    }
    let mut native = PathBuf::from(NATIVE_ROOT);
    native.extend(path.segments().iter().map(|segment| OsStr::from_bytes(segment.as_bytes())));
    Ok(native)
}

pub(crate) fn local_mount() -> Result<MountId, VfsError> {
    Ok(MountId::new(Scheme::new(LOCAL_SCHEME)?, "")?)
}

/// Converts an absolute native path into a `file://` location.
///
/// # Errors
/// `InvalidPath` for relative paths or `..` components. Any bytes are accepted as names.
pub fn storage_path_from_native(native: &Path) -> Result<StoragePath, VfsError> {
    if !native.is_absolute() {
        return Err(VfsError::InvalidPath(PathError::MissingScheme(native.display().to_string())));
    }
    native.components().try_fold(StoragePath::root(local_mount()?), |path, component| match component {
        Component::RootDir | Component::CurDir | Component::Prefix(_) => Ok(path),
        Component::ParentDir => Err(VfsError::InvalidPath(PathError::InvalidSegment("..".to_owned()))),
        Component::Normal(name) => Ok(path.join(PathSegment::from_bytes(name.as_bytes())?)),
    })
}

/// The name of a native path as a segment (`None` for `/` or `..`).
pub(crate) fn segment_of(native: &Path) -> Option<PathSegment> {
    PathSegment::from_bytes(native.file_name()?.as_bytes()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_absolute_paths() {
        let native = Path::new("/Users/me/My Files");
        let path = storage_path_from_native(native).unwrap();
        assert_eq!(path.to_string(), "file:///Users/me/My Files");
        assert_eq!(to_native(&path).unwrap(), native);
    }

    #[test]
    fn non_utf8_names_round_trip() {
        let native = Path::new(OsStr::from_bytes(b"/tmp/caf\xe9"));
        let path = storage_path_from_native(native).unwrap();
        assert_eq!(to_native(&path).unwrap(), native);
    }

    #[test]
    fn rejects_relative_and_foreign_paths() {
        assert!(storage_path_from_native(Path::new("relative")).is_err());
        assert!(to_native(&StoragePath::parse("s3://bucket/key").unwrap()).is_err());
    }
}
