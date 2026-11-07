//! `PermissionEditor` and `SecureEraser` for the local filesystem.
//!
//! Secure erase overwrites a file in place with random data, flushes it to
//! the device, truncates it, renames it to a random name (so the old name
//! doesn't linger in the directory) and removes it. On SSDs and
//! copy-on-write file systems the drive or file system may keep old blocks
//! elsewhere; ADR 0027 and the confirmation dialog say so.

use std::fs::{self, OpenOptions, Permissions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use application::CancelToken;
use application::ports::{PermissionEditor, ProgressCallback, SecureEraser};
use domain::{ByteSize, StoragePath, UnixMode, VfsError};
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::error::from_io;
use crate::lister::LocalFs;
use crate::native_path::to_native;

/// Overwrite unit: large enough for throughput, small enough for progress.
const ERASE_CHUNK: usize = 1024 * 1024;
/// Length of the random hex name a file gets just before removal.
const RANDOM_NAME_BYTES: usize = 12;

impl PermissionEditor for LocalFs {
    fn set_permissions<'a>(&'a self, path: &'a StoragePath, mode: UnixMode) -> BoxFuture<'a, Result<(), VfsError>> {
        async move {
            let native = to_native(path)?;
            self.blocking(move || {
                fs::set_permissions(&native, Permissions::from_mode(mode.bits())).map_err(|e| from_io(&e))
            })
            .await
        }
        .boxed()
    }
}

impl SecureEraser for LocalFs {
    fn erase<'a>(
        &'a self,
        file: &'a StoragePath,
        passes: u8,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<(), VfsError>> {
        async move {
            let native = to_native(file)?;
            self.blocking(move || erase_blocking(&native, passes, &progress, &cancel)).await
        }
        .boxed()
    }
}

fn erase_blocking(path: &Path, passes: u8, progress: &ProgressCallback, cancel: &CancelToken) -> Result<(), VfsError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| from_io(&e))?;
    if metadata.is_dir() {
        return Err(VfsError::IsADirectory);
    }
    if metadata.is_file() {
        overwrite(path, metadata.len(), passes, progress, cancel)?;
    }
    // Links and special files hold no data of their own: just remove them.
    let hidden = path.with_file_name(random_name()?);
    fs::rename(path, &hidden).map_err(|e| from_io(&e))?;
    fs::remove_file(&hidden).map_err(|e| from_io(&e))
}

fn overwrite(
    path: &Path,
    length: u64,
    passes: u8,
    progress: &ProgressCallback,
    cancel: &CancelToken,
) -> Result<(), VfsError> {
    let mut file = OpenOptions::new().write(true).open(path).map_err(|e| from_io(&e))?;
    let mut noise = vec![0_u8; ERASE_CHUNK];
    for _ in 0..passes {
        file.seek(SeekFrom::Start(0)).map_err(|e| from_io(&e))?;
        let mut remaining = length;
        while remaining > 0 {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            let take = usize::try_from(remaining).unwrap_or(usize::MAX).min(ERASE_CHUNK);
            getrandom::fill(&mut noise[..take]).map_err(|e| VfsError::Io { message: e.to_string() })?;
            file.write_all(&noise[..take]).map_err(|e| from_io(&e))?;
            remaining -= take as u64;
            progress(ByteSize(take as u64));
        }
        file.sync_all().map_err(|e| from_io(&e))?;
    }
    file.set_len(0).map_err(|e| from_io(&e))?;
    file.sync_all().map_err(|e| from_io(&e))
}

fn random_name() -> Result<String, VfsError> {
    let mut bytes = [0_u8; RANDOM_NAME_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| VfsError::Io { message: e.to_string() })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn erasing_overwrites_then_removes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.txt");
        fs::write(&path, b"top secret").unwrap();
        let written = Arc::new(AtomicU64::new(0));
        let counter = written.clone();
        let progress: ProgressCallback = Arc::new(move |bytes| {
            counter.fetch_add(bytes.0, Ordering::Relaxed);
        });
        erase_blocking(&path, 2, &progress, &CancelToken::never()).unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "nothing left under another name");
        assert_eq!(written.load(Ordering::Relaxed), 20, "two passes over ten bytes");
    }

    #[test]
    fn folders_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            erase_blocking(dir.path(), 1, &(Arc::new(|_| {}) as ProgressCallback), &CancelToken::never()),
            Err(VfsError::IsADirectory)
        );
    }
}
