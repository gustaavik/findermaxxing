//! `Reader` and `Writer` for local files. Writes go to a hidden temporary file
//! next to the target and appear with a no-replace rename on commit, so a
//! half-written file is never visible under its real name. A checkpointed
//! write keeps its partial file when dropped, so it can be resumed by name.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use application::CancelToken;
use application::ports::{ByteStream, Reader, ResumeState, ResumedSink, WriteSink, Writer};
use domain::{ByteSize, StoragePath, VfsError};
use futures::future::BoxFuture;
use futures::stream;
use futures::{FutureExt, StreamExt};
use tokio::runtime::Handle;

use crate::error::from_io;
use crate::lister::LocalFs;
use crate::native_path::to_native;
use crate::sys::rename_no_replace;

const READ_CHUNK: usize = 1024 * 1024;
const PARTIAL_PREFIX: &str = ".findermaxxing-partial";

/// Distinguishes concurrent partial files in one folder within this process.
static PARTIAL_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Reader for LocalFs {
    async fn read(&self, path: &StoragePath, cancel: CancelToken) -> Result<ByteStream, VfsError> {
        self.read_from(path, ByteSize(0), cancel).await
    }

    async fn read_from(
        &self,
        path: &StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> Result<ByteStream, VfsError> {
        let native = to_native(path)?;
        let file = self
            .blocking(move || {
                if fs::metadata(&native).map_err(|e| from_io(&e))?.is_dir() {
                    return Err(VfsError::IsADirectory);
                }
                let mut file = File::open(&native).map_err(|e| from_io(&e))?;
                file.seek(SeekFrom::Start(offset.0)).map_err(|e| from_io(&e))?;
                Ok(file)
            })
            .await?;
        let runtime = self.runtime().clone();
        let chunks = stream::unfold(Some(file), move |file| {
            let (runtime, cancel) = (runtime.clone(), cancel.clone());
            async move {
                let file = file?;
                if cancel.is_cancelled() {
                    return Some((Err(VfsError::Cancelled), None));
                }
                match runtime.spawn_blocking(move || read_chunk(file)).await {
                    Ok(Ok(Some((chunk, file)))) => Some((Ok(chunk), Some(file))),
                    Ok(Ok(None)) => None,
                    Ok(Err(error)) => Some((Err(error), None)),
                    Err(join) => Some((Err(VfsError::Io { message: join.to_string() }), None)),
                }
            }
        });
        Ok(chunks.boxed())
    }
}

fn read_chunk(mut file: File) -> Result<Option<(Vec<u8>, File)>, VfsError> {
    let mut buffer = vec![0; READ_CHUNK];
    let read = file.read(&mut buffer).map_err(|e| from_io(&e))?;
    if read == 0 {
        return Ok(None);
    }
    buffer.truncate(read);
    Ok(Some((buffer, file)))
}

impl Writer for LocalFs {
    async fn create(&self, path: &StoragePath, _size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        let target = to_native(path)?;
        let runtime = self.runtime().clone();
        self.blocking(move || {
            if fs::symlink_metadata(&target).is_ok() {
                return Err(VfsError::AlreadyExists);
            }
            let partial = partial_path(&target)?;
            let file = OpenOptions::new().write(true).create_new(true).open(&partial).map_err(|e| from_io(&e))?;
            Ok(Box::new(LocalSink { runtime, file: Some(file), partial, target, kept: false }) as Box<dyn WriteSink>)
        })
        .await
    }

    async fn resume(&self, path: &StoragePath, state: &ResumeState) -> Result<ResumedSink, VfsError> {
        let target = to_native(path)?;
        let partial = resumable_partial(&target, state)?;
        let runtime = self.runtime().clone();
        self.blocking(move || {
            if fs::symlink_metadata(&target).is_ok() {
                return Err(VfsError::AlreadyExists);
            }
            let file = OpenOptions::new().append(true).open(&partial).map_err(|e| from_io(&e))?;
            let offset = ByteSize(file.metadata().map_err(|e| from_io(&e))?.len());
            let sink = LocalSink { runtime, file: Some(file), partial, target, kept: true };
            Ok(ResumedSink { sink: Box::new(sink), offset })
        })
        .await
    }

    async fn discard(&self, path: &StoragePath, state: &ResumeState) -> Result<(), VfsError> {
        let partial = resumable_partial(&to_native(path)?, state)?;
        self.blocking(move || match fs::remove_file(&partial) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(from_io(&error)),
            _ => Ok(()),
        })
        .await
    }
}

/// The partial file a state names. Only our own partial names in the
/// target's folder are accepted, so a state can never point elsewhere.
fn resumable_partial(target: &Path, state: &ResumeState) -> Result<PathBuf, VfsError> {
    let name = &state.0;
    let ours = name.starts_with(PARTIAL_PREFIX) && !name.contains(std::path::MAIN_SEPARATOR);
    let parent = target.parent().ok_or(VfsError::NotFound)?;
    if ours { Ok(parent.join(name)) } else { Err(VfsError::NotFound) }
}

fn partial_path(target: &Path) -> Result<PathBuf, VfsError> {
    let parent = target.parent().ok_or(VfsError::NotFound)?;
    let serial = PARTIAL_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!("{PARTIAL_PREFIX}-{}-{serial}", std::process::id())))
}

struct LocalSink {
    runtime: Handle,
    /// `None` only while a write is in flight on the blocking pool.
    file: Option<File>,
    partial: PathBuf,
    target: PathBuf,
    /// Checkpointed: the partial file outlives the sink for resuming.
    kept: bool,
}

impl WriteSink for LocalSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        async move {
            let mut file = self.file.take().ok_or(VfsError::Conflict)?;
            let (file, result) = self
                .runtime
                .spawn_blocking(move || {
                    let result = file.write_all(&chunk).map_err(|e| from_io(&e));
                    (file, result)
                })
                .await
                .map_err(|join| VfsError::Io { message: join.to_string() })?;
            self.file = Some(file);
            result
        }
        .boxed()
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        let (file, partial, target, runtime) =
            (self.file.take(), std::mem::take(&mut self.partial), self.target.clone(), self.runtime.clone());
        async move {
            let task = runtime.spawn_blocking(move || {
                let result = file
                    .ok_or(VfsError::Conflict)
                    .and_then(|file| file.sync_all().map_err(|e| from_io(&e)))
                    .and_then(|()| rename_no_replace(&partial, &target).map_err(|e| from_io(&e)));
                if result.is_err() {
                    let _ = fs::remove_file(&partial);
                }
                result
            });
            task.await.map_err(|join| VfsError::Io { message: join.to_string() })?
        }
        .boxed()
    }

    fn abort(mut self: Box<Self>) -> BoxFuture<'static, ()> {
        // Dropping an unkept sink removes the partial file.
        self.kept = false;
        drop(self);
        futures::future::ready(()).boxed()
    }

    fn checkpoint(&mut self) -> Option<ResumeState> {
        let name = self.partial.file_name()?.to_str()?.to_owned();
        self.kept = true;
        Some(ResumeState(name))
    }
}

impl Drop for LocalSink {
    fn drop(&mut self) {
        if !self.kept && !self.partial.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.partial);
        }
    }
}
