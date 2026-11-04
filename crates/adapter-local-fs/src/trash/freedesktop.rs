//! The freedesktop.org Trash specification 1.0
//! (<https://specifications.freedesktop.org/trash-spec/latest/>).
//!
//! Items go to the home trash when they live on the same device; otherwise
//! to `$topdir/.Trash/$uid` (if the admin provided a sticky `.Trash`) or
//! `$topdir/.Trash-$uid`. Moving is always a same-device rename, never a copy.

use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

const FILES_DIR: &str = "files";
const INFO_DIR: &str = "info";
const INFO_SUFFIX: &str = ".trashinfo";
const SHARED_TRASH: &str = ".Trash";
const TRASH_DIR_MODE: u32 = 0o700;
const STICKY_BIT: u32 = 0o1000;
const DATA_HOME_VARIABLE: &str = "XDG_DATA_HOME";
const HOME_VARIABLE: &str = "HOME";
const DEFAULT_DATA_HOME: &str = ".local/share";
const HOME_TRASH: &str = "Trash";

#[derive(Debug, Clone)]
pub struct FreedesktopTrash {
    home_trash: PathBuf,
    uid: u32,
}

impl FreedesktopTrash {
    /// `$XDG_DATA_HOME/Trash`, defaulting to `~/.local/share/Trash`.
    #[must_use]
    pub fn from_environment() -> Self {
        let data_home =
            std::env::var_os(DATA_HOME_VARIABLE).map(PathBuf::from).filter(|path| path.is_absolute()).unwrap_or_else(
                || PathBuf::from(std::env::var_os(HOME_VARIABLE).unwrap_or_default()).join(DEFAULT_DATA_HOME),
            );
        Self::with_home_trash(data_home.join(HOME_TRASH))
    }

    #[must_use]
    pub fn with_home_trash(home_trash: PathBuf) -> Self {
        // SAFETY: getuid has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        Self { home_trash, uid }
    }

    /// Moves `path` into the right trash directory and returns its new location.
    pub(super) fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        fs::symlink_metadata(path)?;
        let name = path.file_name().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let trash_dir = self.trash_dir_for(path)?;
        let (stored_name, info_path, info_file) = reserve_slot(&trash_dir, name)?;
        write_info(info_file, path)?;
        let destination = trash_dir.join(FILES_DIR).join(&stored_name);
        if let Err(error) = fs::rename(path, &destination) {
            let _ = fs::remove_file(&info_path);
            return Err(error);
        }
        Ok(destination)
    }

    /// Drops the `.trashinfo` of an item that has been restored.
    pub(super) fn forget(&self, trashed: &Path) {
        let (Some(name), Some(trash_dir)) = (trashed.file_name(), trashed.parent().and_then(Path::parent)) else {
            return;
        };
        let _ = fs::remove_file(trash_dir.join(INFO_DIR).join(info_name(name)));
    }

    fn trash_dir_for(&self, path: &Path) -> io::Result<PathBuf> {
        let parent = path.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let device = fs::metadata(parent)?.dev();
        ensure_private_dir(&self.home_trash)?;
        if fs::metadata(&self.home_trash)?.dev() == device {
            return Ok(self.home_trash.clone());
        }
        self.volume_trash(&top_directory(parent, device)?)
    }

    fn volume_trash(&self, topdir: &Path) -> io::Result<PathBuf> {
        let shared = topdir.join(SHARED_TRASH);
        if is_valid_shared_trash(&shared) {
            let own = shared.join(self.uid.to_string());
            ensure_private_dir(&own)?;
            return Ok(own);
        }
        let own = topdir.join(format!("{SHARED_TRASH}-{}", self.uid));
        ensure_private_dir(&own)?;
        let metadata = fs::symlink_metadata(&own)?;
        if metadata.file_type().is_symlink() || metadata.uid() != self.uid {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(own)
    }
}

/// The highest ancestor of `dir` still on `device`: the mount point.
fn top_directory(dir: &Path, device: u64) -> io::Result<PathBuf> {
    let mut top = dir;
    while let Some(up) = top.parent() {
        if fs::metadata(up)?.dev() != device {
            break;
        }
        top = up;
    }
    Ok(top.to_path_buf())
}

/// The spec only trusts an admin-made `$topdir/.Trash` that is a real, sticky directory.
fn is_valid_shared_trash(dir: &Path) -> bool {
    fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && m.mode() & STICKY_BIT != 0)
}

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match DirBuilder::new().recursive(true).mode(TRASH_DIR_MODE).create(dir) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

/// Atomically claims a free name by creating its `.trashinfo` with `O_EXCL`.
fn reserve_slot(trash_dir: &Path, name: &OsStr) -> io::Result<(OsString, PathBuf, File)> {
    let (files, info) = (trash_dir.join(FILES_DIR), trash_dir.join(INFO_DIR));
    ensure_private_dir(&files)?;
    ensure_private_dir(&info)?;
    for attempt in 1_u64.. {
        let candidate = numbered(name, attempt);
        if fs::symlink_metadata(files.join(&candidate)).is_ok() {
            continue;
        }
        let info_path = info.join(info_name(&candidate));
        match OpenOptions::new().write(true).create_new(true).open(&info_path) {
            Ok(file) => return Ok((candidate, info_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

fn numbered(name: &OsStr, attempt: u64) -> OsString {
    if attempt == 1 {
        return name.to_owned();
    }
    let mut bytes = name.as_bytes().to_vec();
    bytes.extend_from_slice(format!(".{attempt}").as_bytes());
    OsString::from_vec(bytes)
}

fn info_name(stored: &OsStr) -> OsString {
    let mut bytes = stored.as_bytes().to_vec();
    bytes.extend_from_slice(INFO_SUFFIX.as_bytes());
    OsString::from_vec(bytes)
}

fn write_info(mut file: File, original: &Path) -> io::Result<()> {
    let contents = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode(original.as_os_str().as_bytes()),
        local_timestamp()
    );
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

/// RFC 2396 escaping as the spec requires; `/` stays literal.
fn percent_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `YYYY-MM-DDThh:mm:ss` in local time, as the spec requires.
fn local_timestamp() -> String {
    // SAFETY: `time` accepts a null pointer; `localtime_r` writes only into the
    // zero-initialised `tm` we own, and returns null on failure, which we check.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&raw const now, &raw mut tm).is_null() {
            return String::new();
        }
        tm
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, FreedesktopTrash) {
        let root = tempfile::tempdir().unwrap();
        let trash = FreedesktopTrash::with_home_trash(root.path().join("Trash"));
        (root, trash)
    }

    #[test]
    fn trash_writes_spec_info_and_forget_removes_it() {
        let (root, trash) = setup();
        let item = root.path().join("my report.pdf");
        fs::write(&item, b"x").unwrap();

        let trashed = trash.trash(&item).unwrap();

        assert!(!item.exists());
        assert_eq!(trashed, root.path().join("Trash/files/my report.pdf"));
        let info = fs::read_to_string(root.path().join("Trash/info/my report.pdf.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=/"));
        assert!(info.contains("my%20report.pdf\n"));
        assert!(info.contains("DeletionDate="));

        trash.forget(&trashed);
        assert!(!root.path().join("Trash/info/my report.pdf.trashinfo").exists());
    }

    #[test]
    fn same_name_twice_gets_distinct_slots() {
        let (root, trash) = setup();
        let item = root.path().join("a.txt");
        fs::write(&item, b"1").unwrap();
        let first = trash.trash(&item).unwrap();
        fs::write(&item, b"2").unwrap();
        let second = trash.trash(&item).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(second).unwrap(), b"2");
        assert!(root.path().join("Trash/info/a.txt.2.trashinfo").exists());
    }

    #[test]
    fn encoding_keeps_slashes_and_escapes_the_rest() {
        assert_eq!(percent_encode(b"/a b/caf\xc3\xa9%"), "/a%20b/caf%C3%A9%25");
    }
}
