//! The two syscalls std does not expose: atomic no-replace rename and (on
//! macOS) copy-on-write cloning.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Renames without ever replacing an existing destination, atomically.
///
/// A destination that is the *same* file under different letter case
/// (`a.JPG` → `a.jpg` on a case-insensitive volume) is a plain rename.
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    match exclusive_rename(from, to) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && same_file(from, to) => std::fs::rename(from, to),
        other => other,
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(target_os = "macos")]
fn exclusive_rename(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (c_path(from)?, c_path(to)?);
    // SAFETY: both arguments are valid NUL-terminated strings that outlive the call.
    check(unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) })
}

#[cfg(target_os = "linux")]
fn exclusive_rename(from: &Path, to: &Path) -> io::Result<()> {
    let (c_from, c_to) = (c_path(from)?, c_path(to)?);
    // SAFETY: both arguments are valid NUL-terminated strings that outlive the call.
    let result = check(unsafe {
        libc::renameat2(libc::AT_FDCWD, c_from.as_ptr(), libc::AT_FDCWD, c_to.as_ptr(), libc::RENAME_NOREPLACE)
    });
    match result {
        // Some filesystems (older NFS, some FUSE) lack RENAME_NOREPLACE: check, then rename.
        // The window between the two is unavoidable there.
        Err(error) if error.raw_os_error() == Some(libc::EINVAL) => {
            if std::fs::symlink_metadata(to).is_ok() {
                return Err(io::Error::from(io::ErrorKind::AlreadyExists));
            }
            std::fs::rename(from, to)
        }
        other => other,
    }
}

/// `CLONE_NOFOLLOW` from `<sys/clonefile.h>` (not exported by the libc crate):
/// clone a symlink itself rather than its target.
#[cfg(target_os = "macos")]
const CLONE_NOFOLLOW: u32 = 0x0001;

/// Copy-on-write clone (APFS). `Ok(false)` means the volume can't clone and
/// the caller should copy bytes instead.
#[cfg(target_os = "macos")]
pub(crate) fn try_clone_file(from: &Path, to: &Path) -> io::Result<bool> {
    let (c_from, c_to) = (c_path(from)?, c_path(to)?);
    // SAFETY: both arguments are valid NUL-terminated strings that outlive the call.
    match check(unsafe { libc::clonefile(c_from.as_ptr(), c_to.as_ptr(), CLONE_NOFOLLOW) }) {
        Ok(()) => Ok(true),
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOTSUP | libc::EXDEV)) => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn try_clone_file(_from: &Path, _to: &Path) -> io::Result<bool> {
    Ok(false)
}
