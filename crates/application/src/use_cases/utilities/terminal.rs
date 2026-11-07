//! Opening the built-in terminal in the folder being viewed.

use std::path::PathBuf;
use std::sync::Arc;

use domain::StoragePath;

use super::checksums::opaque_debug;
use crate::ports::{ScreenChanged, TerminalError, TerminalLauncher, TerminalSession, TerminalSize};

/// The scheme whose paths are paths on this machine.
const LOCAL_SCHEME: &str = "file";

pub struct OpenTerminal {
    launcher: Arc<dyn TerminalLauncher>,
    /// Where shells start for folders that aren't on this machine.
    home: PathBuf,
}

impl OpenTerminal {
    #[must_use]
    pub fn new(launcher: Arc<dyn TerminalLauncher>, home: PathBuf) -> Self {
        Self { launcher, home }
    }

    /// Starts a shell in `folder` (in the home folder when `folder` is on a
    /// server, bucket or drive).
    ///
    /// # Errors
    /// The shell couldn't be started.
    pub fn execute(
        &self,
        folder: &StoragePath,
        size: TerminalSize,
        changed: ScreenChanged,
    ) -> Result<Arc<dyn TerminalSession>, TerminalError> {
        let start = local_folder(folder).unwrap_or_else(|| self.home.clone());
        self.launcher.open(start, size, changed)
    }
}

/// The path on this machine of a local folder.
#[must_use]
pub fn local_folder(folder: &StoragePath) -> Option<PathBuf> {
    if folder.mount().scheme().as_str() != LOCAL_SCHEME {
        return None;
    }
    let mut path = PathBuf::from("/");
    for segment in folder.segments() {
        path.push(native_segment(segment.as_bytes()));
    }
    Some(path)
}

#[cfg(unix)]
fn native_segment(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn native_segment(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

opaque_debug!(OpenTerminal);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_folders_have_a_native_path() {
        let local = StoragePath::parse("file:///Users/me/Projects").unwrap();
        assert_eq!(local_folder(&local), Some(PathBuf::from("/Users/me/Projects")));
        let remote = StoragePath::parse("s3://acct/bucket/x").unwrap();
        assert_eq!(local_folder(&remote), None);
    }
}
