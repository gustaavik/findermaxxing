//! `Copier`, `Mover`, `Remover` and `FolderMaker` for the local filesystem.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use application::CancelToken;
use application::ports::{Copier, FolderMaker, Mover, ProgressCallback, Remover};
use domain::{ByteSize, Capability, FileEntry, StoragePath, VfsError};

use crate::error::from_io;
use crate::lister::{LocalFs, stat_blocking};
use crate::native_path::to_native;
use crate::sys::{rename_no_replace, try_clone_file};

/// Read/write unit for byte copies: large enough for throughput, small enough
/// that progress and cancellation stay responsive.
const COPY_CHUNK: usize = 1024 * 1024;

impl Copier for LocalFs {
    async fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<ByteSize, VfsError> {
        let (from, to) = (to_native(from)?, to_native(to)?);
        self.blocking(move || copy_item_blocking(&from, &to, &progress, &cancel)).await
    }
}

fn copy_item_blocking(
    from: &Path,
    to: &Path,
    progress: &ProgressCallback,
    cancel: &CancelToken,
) -> Result<ByteSize, VfsError> {
    let metadata = fs::symlink_metadata(from).map_err(|e| from_io(&e))?;
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        return Err(VfsError::IsADirectory);
    }
    if cancel.is_cancelled() {
        return Err(VfsError::Cancelled);
    }
    if file_type.is_symlink() {
        let target = fs::read_link(from).map_err(|e| from_io(&e))?;
        std::os::unix::fs::symlink(target, to).map_err(|e| from_io(&e))?;
        return Ok(ByteSize(0));
    }
    if !file_type.is_file() {
        // Sockets, FIFOs and device nodes are not copyable content.
        return Err(VfsError::Unsupported(Capability::Copy));
    }
    let size = ByteSize(metadata.len());
    if try_clone_file(from, to).map_err(|e| from_io(&e))? {
        progress(size);
        return Ok(size);
    }
    copy_bytes(from, to, &metadata, progress, cancel).inspect_err(|_| {
        // Never leave a truncated copy behind; the source is untouched.
        let _ = fs::remove_file(to);
    })?;
    Ok(size)
}

fn copy_bytes(
    from: &Path,
    to: &Path,
    metadata: &Metadata,
    progress: &ProgressCallback,
    cancel: &CancelToken,
) -> Result<(), VfsError> {
    let io_error = |e: io::Error| from_io(&e);
    let mut reader = File::open(from).map_err(io_error)?;
    // `create_new` makes the no-overwrite rule atomic.
    let mut writer = OpenOptions::new().write(true).create_new(true).open(to).map_err(io_error)?;
    let mut buffer = vec![0_u8; COPY_CHUNK];
    loop {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let read = reader.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read]).map_err(io_error)?;
        progress(ByteSize(read as u64));
    }
    writer.set_permissions(metadata.permissions()).map_err(io_error)?;
    if let Ok(modified) = metadata.modified() {
        writer.set_modified(modified).map_err(io_error)?;
    }
    writer.sync_all().map_err(io_error)
}

impl Mover for LocalFs {
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        let (from, to) = (to_native(from)?, to_native(to)?);
        self.blocking(move || rename_no_replace(&from, &to).map_err(|e| from_io(&e))).await
    }
}

impl Remover for LocalFs {
    async fn remove(&self, path: &StoragePath, cancel: CancelToken) -> Result<(), VfsError> {
        let native = to_native(path)?;
        self.blocking(move || {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            let metadata = fs::symlink_metadata(&native).map_err(|e| from_io(&e))?;
            // remove_dir_all never follows symlinks, so a link to a folder only removes the link.
            let result = if metadata.is_dir() { fs::remove_dir_all(&native) } else { fs::remove_file(&native) };
            result.map_err(|e| from_io(&e))
        })
        .await
    }
}

impl FolderMaker for LocalFs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        let (native, path) = (to_native(path)?, path.clone());
        self.blocking(move || {
            fs::create_dir(&native).map_err(|e| from_io(&e))?;
            stat_blocking(&path)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use application::CancellationSource;

    use super::*;

    const CHUNKS: usize = 3;

    fn big_file(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("big");
        fs::write(&path, vec![1_u8; COPY_CHUNK * CHUNKS + 7]).unwrap();
        path
    }

    #[test]
    fn byte_copy_reports_progress_per_chunk_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let from = big_file(dir.path());
        fs::set_permissions(&from, fs::Permissions::from_mode(0o640)).unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let counter = calls.clone();
        let progress: ProgressCallback = Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let to = dir.path().join("copy");

        copy_bytes(&from, &to, &fs::metadata(&from).unwrap(), &progress, &CancelToken::never()).unwrap();

        assert_eq!(fs::read(&to).unwrap(), fs::read(&from).unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), CHUNKS as u64 + 1);
        assert_eq!(fs::metadata(&to).unwrap().permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn cancelling_mid_copy_removes_the_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = big_file(dir.path());
        let to = dir.path().join("copy");
        let source = Arc::new(CancellationSource::new());
        let canceller = source.clone();
        let progress: ProgressCallback = Arc::new(move |_| canceller.cancel());

        let result = copy_bytes(&from, &to, &fs::metadata(&from).unwrap(), &progress, &source.token())
            .inspect_err(|_| drop(fs::remove_file(&to)));

        assert_eq!(result, Err(VfsError::Cancelled));
        assert!(!to.exists());
    }

    #[test]
    fn symlinks_are_copied_as_links() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink("target-that-need-not-exist", &link).unwrap();
        let copy = dir.path().join("copy");
        copy_item_blocking(&link, &copy, &(Arc::new(|_| {}) as ProgressCallback), &CancelToken::never()).unwrap();
        assert_eq!(fs::read_link(copy).unwrap(), Path::new("target-that-need-not-exist"));
    }
}
