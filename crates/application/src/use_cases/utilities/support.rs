//! Small steps the utilities share: local copies, free names, and writing
//! new items from a byte stream.

use domain::{ByteSize, EntryKind, FIRST_DUPLICATE_NUMBER, PathSegment, StoragePath, VfsError, numbered_name};
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::cancel::CancelToken;
use crate::ports::{LocalFile, MountHandle, MountResolver, ProgressCallback, WriteSink};

/// A local file for `item`: the item itself on local disks, a (cached)
/// download elsewhere.
pub(crate) async fn local_copy(mounts: &dyn MountResolver, item: &StoragePath) -> Result<LocalFile, VfsError> {
    let mount = mounts.resolve(item.mount())?;
    mount.materializer()?.local_file_boxed(item).await
}

/// `folder/name`, or the first numbered variant (`name 2`) that is free.
pub(crate) async fn free_path(
    mount: &MountHandle,
    folder: &StoragePath,
    name: &PathSegment,
    kind: EntryKind,
) -> Result<StoragePath, VfsError> {
    let lister = mount.lister()?;
    let mut candidate = folder.join(name.clone());
    let mut number = FIRST_DUPLICATE_NUMBER;
    loop {
        match lister.stat_boxed(&candidate).await {
            Err(VfsError::NotFound) => return Ok(candidate),
            Err(error) => return Err(error),
            Ok(_) => {
                candidate = folder.join(numbered_name(name, kind, number)?);
                number = number.saturating_add(1);
            }
        }
    }
}

/// A new folder at the first free variant of `folder/name`.
pub(crate) async fn create_free_folder(
    mount: &MountHandle,
    folder: &StoragePath,
    name: &PathSegment,
) -> Result<StoragePath, VfsError> {
    let maker = mount.folder_maker()?;
    loop {
        let path = free_path(mount, folder, name, EntryKind::Directory).await?;
        match maker.create_folder_boxed(&path).await {
            // Someone took the name in between: look again.
            Err(VfsError::AlreadyExists) => {}
            Err(error) => return Err(error),
            Ok(_) => return Ok(path),
        }
    }
}

/// Writes `chunks` to a new file at the first free variant of
/// `folder/name`; nothing is left behind on failure.
pub(crate) async fn write_new_file(
    mount: &MountHandle,
    folder: &StoragePath,
    name: &PathSegment,
    chunks: BoxStream<'_, Result<Vec<u8>, VfsError>>,
    progress: &ProgressCallback,
    cancel: &CancelToken,
) -> Result<StoragePath, VfsError> {
    let path = free_path(mount, folder, name, EntryKind::File).await?;
    let mut sink = mount.writer()?.create_boxed(&path, None).await?;
    match pump(&mut *sink, chunks, progress, cancel).await {
        Ok(()) => sink.commit().await.map(|()| path),
        Err(error) => {
            sink.abort().await;
            Err(error)
        }
    }
}

/// Copies every chunk into `sink`; the caller commits or aborts.
pub(crate) async fn pump(
    sink: &mut dyn WriteSink,
    mut chunks: BoxStream<'_, Result<Vec<u8>, VfsError>>,
    progress: &ProgressCallback,
    cancel: &CancelToken,
) -> Result<(), VfsError> {
    while let Some(chunk) = chunks.next().await {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let chunk = chunk?;
        let length = chunk.len() as u64;
        sink.write(chunk).await?;
        progress(ByteSize(length));
    }
    Ok(())
}

/// One chunk as a stream.
pub(crate) fn once(bytes: Vec<u8>) -> BoxStream<'static, Result<Vec<u8>, VfsError>> {
    futures::stream::once(async move { Ok(bytes) }).boxed()
}

/// For writes whose progress nobody shows.
pub(crate) fn no_progress() -> ProgressCallback {
    std::sync::Arc::new(|_| {})
}
