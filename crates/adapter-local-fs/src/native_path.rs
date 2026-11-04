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
    native.extend(path.segments().iter().map(PathSegment::as_str));
    Ok(native)
}

pub(crate) fn local_mount() -> Result<MountId, VfsError> {
    Ok(MountId::new(Scheme::new(LOCAL_SCHEME)?, "")?)
}

/// Converts an absolute native path into a `file://` location.
///
/// # Errors
/// `InvalidPath` for relative paths, `..` components or non-UTF-8 names.
pub fn storage_path_from_native(native: &Path) -> Result<StoragePath, VfsError> {
    if !native.is_absolute() {
        return Err(VfsError::InvalidPath(PathError::MissingScheme(native.display().to_string())));
    }
    native.components().try_fold(StoragePath::root(local_mount()?), |path, component| match component {
        Component::RootDir | Component::CurDir | Component::Prefix(_) => Ok(path),
        Component::ParentDir => Err(VfsError::InvalidPath(PathError::InvalidSegment("..".to_owned()))),
        Component::Normal(name) => {
            let name = name.to_str().ok_or_else(|| non_utf8(native))?;
            Ok(path.join(PathSegment::new(name)?))
        }
    })
}

pub(crate) fn non_utf8(native: &Path) -> VfsError {
    VfsError::InvalidPath(PathError::InvalidSegment(native.display().to_string()))
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
    fn rejects_relative_and_foreign_paths() {
        assert!(storage_path_from_native(Path::new("relative")).is_err());
        assert!(to_native(&StoragePath::parse("s3://bucket/key").unwrap()).is_err());
    }
}
