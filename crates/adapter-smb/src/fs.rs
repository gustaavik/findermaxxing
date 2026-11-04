//! The SMB roles for one share (or a folder in it). Writes go to a hidden
//! partial file that is renamed onto the target without replacing on commit;
//! a checkpointed partial survives a crash and is continued at its size.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use application::CancelToken;
use application::ports::{
    ByteStream, Copier, FolderMaker, Lister, Mover, ProgressCallback, Reader, Remover, ResumeState, ResumedSink,
    WriteSink, Writer,
};
use domain::{
    ByteSize, EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, PathError, PathSegment, StoragePath,
    Timestamp, VfsError, Visibility,
};
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use futures::{FutureExt, StreamExt};
use smb2::client::stream::{FileReader, FileWriter};
use smb2::{FileAttributes, RenameOptions, SmbClient, Tree};
use tokio::runtime::Handle;
use tokio::sync::Mutex;

use crate::errors::from_smb;

const READ_CHUNK: u64 = 1024 * 1024;
const SEPARATOR: char = '/';
const HIDDEN_PREFIX: u8 = b'.';
const PARTIAL_SUFFIX: &str = "findermaxxing-partial";
const DOT_ENTRIES: [&str; 2] = [".", ".."];

static PARTIALS: AtomicU64 = AtomicU64::new(0);

/// One connected share. Metadata calls take turns on the client; streaming
/// readers and writers run independently over the same session.
#[derive(Clone)]
pub struct SmbFs {
    client: Arc<Mutex<SmbClient>>,
    tree: Tree,
    /// Folder within the share that is the mount's root (`""` = the share).
    base: String,
    root: StoragePath,
    runtime: Handle,
}

impl std::fmt::Debug for SmbFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmbFs").field("base", &self.base).finish_non_exhaustive()
    }
}

/// What a name or listing says about an item.
struct Facts {
    is_directory: bool,
    size: u64,
    modified: smb2::pack::FileTime,
    attributes: FileAttributes,
}

impl SmbFs {
    pub(crate) fn new(
        client: Arc<Mutex<SmbClient>>,
        tree: Tree,
        base: String,
        root: StoragePath,
        runtime: Handle,
    ) -> Self {
        Self { client, tree, base, root, runtime }
    }

    #[must_use]
    pub fn root(&self) -> &StoragePath {
        &self.root
    }

    /// SMB names are Unicode: a name that isn't UTF-8 can't be addressed.
    fn remote(&self, location: &StoragePath) -> Result<String, VfsError> {
        let mut parts: Vec<&str> = self.base.split(SEPARATOR).filter(|part| !part.is_empty()).collect();
        for segment in location.segments() {
            parts.push(
                std::str::from_utf8(segment.as_bytes()).map_err(|_| {
                    VfsError::InvalidPath(PathError::InvalidSegment(segment.display_name().into_owned()))
                })?,
            );
        }
        Ok(parts.join("/"))
    }

    async fn facts(&self, location: &StoragePath) -> Result<Facts, VfsError> {
        let path = self.remote(location)?;
        let mut tree = self.tree.clone();
        let info = self.client.lock().await.stat(&mut tree, &path).await.map_err(|e| from_smb(&e))?;
        Ok(Facts {
            is_directory: info.is_directory,
            size: info.size,
            modified: info.modified,
            attributes: info.attributes,
        })
    }

    async fn exists(&self, location: &StoragePath) -> Result<bool, VfsError> {
        match self.facts(location).await {
            Ok(_) => Ok(true),
            Err(VfsError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn require_folder(&self, location: &StoragePath) -> Result<(), VfsError> {
        if location.parent().is_none() {
            return Ok(());
        }
        if self.facts(location).await?.is_directory { Ok(()) } else { Err(VfsError::NotADirectory) }
    }

    fn entry(parent: &StoragePath, name: PathSegment, facts: &Facts) -> FileEntry {
        let kind = if facts.is_directory { EntryKind::Directory } else { EntryKind::File };
        let hidden = name.as_bytes().first() == Some(&HIDDEN_PREFIX) || facts.attributes.is_hidden();
        let modified = facts.modified.to_system_time().and_then(|time| time.duration_since(UNIX_EPOCH).ok());
        let metadata = EntryMetadata {
            size: (kind == EntryKind::File).then_some(ByteSize(facts.size)),
            modified: modified
                .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
                .map(Timestamp::from_unix_millis),
            visibility: if hidden { Visibility::Hidden } else { Visibility::Visible },
            permissions: None,
        };
        FileEntry::new(parent, name, kind, metadata)
    }

    async fn children(&self, dir: &StoragePath) -> Result<Vec<FileEntry>, VfsError> {
        let path = self.remote(dir)?;
        let mut tree = self.tree.clone();
        let listed = self.client.lock().await.list_directory(&mut tree, &path).await.map_err(|e| from_smb(&e));
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                return Err(match self.facts(dir).await {
                    Ok(facts) if !facts.is_directory => VfsError::NotADirectory,
                    Ok(_) => error,
                    Err(stat_error) => stat_error,
                });
            }
        };
        if dir.parent().is_some() && !self.facts(dir).await?.is_directory {
            return Err(VfsError::NotADirectory);
        }
        listed
            .into_iter()
            .filter(|item| !DOT_ENTRIES.contains(&item.name.as_str()))
            .map(|item| {
                let facts = Facts {
                    is_directory: item.is_directory,
                    size: item.size,
                    modified: item.modified,
                    attributes: item.attributes,
                };
                Ok(Self::entry(dir, PathSegment::new(&item.name)?, &facts))
            })
            .collect()
    }

    async fn delete(&self, location: &StoragePath, is_directory: bool) -> Result<(), VfsError> {
        let path = self.remote(location)?;
        let mut client = self.client.lock().await;
        let result = if is_directory {
            client.delete_directory(&self.tree, &path).await
        } else {
            client.delete_file(&self.tree, &path).await
        };
        result.map_err(|e| from_smb(&e))
    }

    async fn remove_tree(&self, location: &StoragePath, cancel: &CancelToken) -> Result<(), VfsError> {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let facts = self.facts(location).await?;
        if facts.is_directory {
            for child in self.children(location).await? {
                Box::pin(self.remove_tree(child.path(), cancel)).await?;
            }
        }
        self.delete(location, facts.is_directory).await
    }

    fn partial_for(&self, target: &StoragePath, state: &ResumeState) -> Result<StoragePath, VfsError> {
        let parent = target.parent().ok_or(VfsError::NotFound)?;
        let ours = state.0.starts_with(char::from(HIDDEN_PREFIX)) && state.0.contains(PARTIAL_SUFFIX);
        if !ours || state.0.contains(SEPARATOR) {
            return Err(VfsError::NotFound);
        }
        Ok(parent.join(PathSegment::new(&state.0)?))
    }

    async fn rename_no_replace(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        let (from, to) = (self.remote(from)?, self.remote(to)?);
        let mut client = self.client.lock().await;
        client.rename_with(&self.tree, &from, &to, RenameOptions::new()).await.map_err(|e| from_smb(&e))
    }
}

impl Lister for SmbFs {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let (fs, dir) = (self.clone(), dir.clone());
        stream::once(async move {
            let batches: Vec<Result<EntryBatch, VfsError>> = match fs.children(&dir).await {
                Ok(_) if cancel.is_cancelled() => vec![Err(VfsError::Cancelled)],
                Ok(entries) => entries.chunks(LIST_BATCH_SIZE).map(|chunk| Ok(chunk.to_vec())).collect(),
                Err(error) => vec![Err(error)],
            };
            stream::iter(batches)
        })
        .flatten()
    }

    async fn stat(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        let (parent, name) = path.parent().zip(path.name().cloned()).ok_or(VfsError::InvalidPath(PathError::Root))?;
        let facts = self.facts(path).await?;
        Ok(Self::entry(&parent, name, &facts))
    }
}

impl Reader for SmbFs {
    async fn read(&self, path: &StoragePath, cancel: CancelToken) -> Result<ByteStream, VfsError> {
        self.read_from(path, ByteSize(0), cancel).await
    }

    async fn read_from(
        &self,
        path: &StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> Result<ByteStream, VfsError> {
        if self.facts(path).await?.is_directory {
            return Err(VfsError::IsADirectory);
        }
        let remote = self.remote(path)?;
        let reader = self.client.lock().await.open_file_reader(&self.tree, &remote).await.map_err(|e| from_smb(&e))?;
        let size = reader.size();
        let chunks = stream::unfold(Some((reader, offset.0)), move |state| {
            let cancel = cancel.clone();
            async move {
                let (reader, position): (FileReader, u64) = state?;
                if cancel.is_cancelled() {
                    let _ = reader.close().await;
                    return Some((Err(VfsError::Cancelled), None));
                }
                if position >= size {
                    let _ = reader.close().await;
                    return None;
                }
                match reader.read_at(position, READ_CHUNK.min(size - position)).await {
                    Ok(bytes) if bytes.is_empty() => {
                        let _ = reader.close().await;
                        None
                    }
                    Ok(bytes) => {
                        let next = position + bytes.len() as u64;
                        Some((Ok(bytes), Some((reader, next))))
                    }
                    Err(error) => Some((Err(from_smb(&error)), None)),
                }
            }
        });
        Ok(chunks.boxed())
    }
}

fn partial_name(target: &StoragePath) -> Result<PathSegment, VfsError> {
    let name = target.name().ok_or(VfsError::NotFound)?;
    let serial = PARTIALS.fetch_add(1, Ordering::Relaxed);
    let mut bytes = vec![HIDDEN_PREFIX];
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(format!(".{PARTIAL_SUFFIX}-{}-{serial}", std::process::id()).as_bytes());
    PathSegment::from_bytes(&bytes).map_err(VfsError::from)
}

impl Writer for SmbFs {
    async fn create(&self, path: &StoragePath, _size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        if self.exists(path).await? {
            return Err(VfsError::AlreadyExists);
        }
        let parent = path.parent().ok_or(VfsError::NotFound)?;
        self.require_folder(&parent).await?;
        let partial = parent.join(partial_name(path)?);
        let remote = self.remote(&partial)?;
        let writer = self
            .client
            .lock()
            .await
            .create_file_writer_exclusive(&self.tree, &remote)
            .await
            .map_err(|e| from_smb(&e))?;
        Ok(Box::new(SmbSink { fs: self.clone(), target: path.clone(), partial, writer: Some(writer), kept: false }))
    }

    async fn resume(&self, path: &StoragePath, state: &ResumeState) -> Result<ResumedSink, VfsError> {
        if self.exists(path).await? {
            return Err(VfsError::AlreadyExists);
        }
        let partial = self.partial_for(path, state)?;
        let size = self.facts(&partial).await?.size;
        let remote = self.remote(&partial)?;
        let writer = self
            .client
            .lock()
            .await
            .create_file_writer_at(&self.tree, &remote, size)
            .await
            .map_err(|e| from_smb(&e))?;
        let sink = SmbSink { fs: self.clone(), target: path.clone(), partial, writer: Some(writer), kept: true };
        Ok(ResumedSink { sink: Box::new(sink), offset: ByteSize(size) })
    }

    async fn discard(&self, path: &StoragePath, state: &ResumeState) -> Result<(), VfsError> {
        let partial = self.partial_for(path, state)?;
        match self.delete(&partial, false).await {
            Ok(()) | Err(VfsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

struct SmbSink {
    fs: SmbFs,
    target: StoragePath,
    partial: StoragePath,
    /// `None` once committed or aborted.
    writer: Option<FileWriter>,
    /// Checkpointed: the partial file outlives the sink for resuming.
    kept: bool,
}

impl SmbSink {
    fn remove_partial(&self) -> BoxFuture<'static, ()> {
        let (fs, partial) = (self.fs.clone(), self.partial.clone());
        async move {
            let _ = fs.delete(&partial, false).await;
        }
        .boxed()
    }
}

impl WriteSink for SmbSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        async move {
            let writer = self.writer.as_mut().ok_or(VfsError::Conflict)?;
            writer.write_chunk(&chunk).await.map_err(|e| from_smb(&e))
        }
        .boxed()
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        async move {
            let writer = self.writer.take().ok_or(VfsError::Conflict)?;
            let result = async {
                writer.finish().await.map_err(|e| from_smb(&e))?;
                self.fs.rename_no_replace(&self.partial, &self.target).await
            }
            .await;
            if result.is_err() {
                self.remove_partial().await;
            }
            self.kept = true;
            result
        }
        .boxed()
    }

    fn abort(mut self: Box<Self>) -> BoxFuture<'static, ()> {
        let writer = self.writer.take();
        self.kept = true;
        let remove = self.remove_partial();
        async move {
            if let Some(writer) = writer {
                let _ = writer.abort().await;
            }
            remove.await;
        }
        .boxed()
    }

    fn checkpoint(&mut self) -> Option<ResumeState> {
        let name = std::str::from_utf8(self.partial.name()?.as_bytes()).ok()?.to_owned();
        self.kept = true;
        Some(ResumeState(name))
    }
}

impl Drop for SmbSink {
    fn drop(&mut self) {
        if !self.kept {
            let writer = self.writer.take();
            let remove = self.remove_partial();
            self.fs.runtime.spawn(async move {
                if let Some(writer) = writer {
                    let _ = writer.abort().await;
                }
                remove.await;
            });
        }
    }
}

impl Copier for SmbFs {
    /// Streams through this machine; server-side copy (`FSCTL_SRV_COPYCHUNK`)
    /// is a later optimisation.
    async fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<ByteSize, VfsError> {
        if self.facts(from).await?.is_directory {
            return Err(VfsError::IsADirectory);
        }
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let mut chunks = self.read(from, cancel.clone()).await?;
        let mut sink = self.create(to, None).await?;
        let mut copied = 0_u64;
        while let Some(chunk) = chunks.next().await {
            let written = match chunk {
                Ok(_) if cancel.is_cancelled() => Err(VfsError::Cancelled),
                Ok(chunk) => {
                    let length = chunk.len() as u64;
                    sink.write(chunk).await.map(|()| length)
                }
                Err(error) => Err(error),
            };
            match written {
                Ok(length) => {
                    copied += length;
                    progress(ByteSize(length));
                }
                Err(error) => {
                    sink.abort().await;
                    return Err(error);
                }
            }
        }
        sink.commit().await.map(|()| ByteSize(copied))
    }
}

impl Mover for SmbFs {
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        self.facts(from).await?;
        let same_item = from.parent() == to.parent()
            && from.name().map(|n| n.display_name().to_lowercase())
                == to.name().map(|n| n.display_name().to_lowercase());
        if !same_item && self.exists(to).await? {
            return Err(VfsError::AlreadyExists);
        }
        self.require_folder(&to.parent().ok_or(VfsError::NotFound)?).await?;
        self.rename_no_replace(from, to).await
    }
}

impl Remover for SmbFs {
    async fn remove(&self, path: &StoragePath, cancel: CancelToken) -> Result<(), VfsError> {
        self.remove_tree(path, &cancel).await
    }
}

impl FolderMaker for SmbFs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        if self.exists(path).await? {
            return Err(VfsError::AlreadyExists);
        }
        self.require_folder(&path.parent().ok_or(VfsError::InvalidPath(PathError::Root))?).await?;
        let remote = self.remote(path)?;
        self.client.lock().await.create_directory(&self.tree, &remote).await.map_err(|e| from_smb(&e))?;
        self.stat(path).await
    }
}
