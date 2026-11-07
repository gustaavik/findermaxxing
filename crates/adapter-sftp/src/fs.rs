//! The SFTP roles for one account. Writes go to a hidden partial file that
//! is renamed onto the target on commit; SFTP's rename never replaces an
//! existing item, so nothing is overwritten. A checkpointed partial file
//! survives a crash and is continued by appending.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use application::CancelToken;
use application::ports::{
    ByteStream, Copier, FolderMaker, Lister, Mover, PermissionEditor, ProgressCallback, Reader, Remover, ResumeState,
    ResumedSink, WriteSink, Writer,
};
use domain::{
    ByteSize, EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, PathError, PathSegment, StoragePath,
    SymlinkTarget, Timestamp, UnixMode, VfsError, Visibility,
};
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use futures::{FutureExt, StreamExt};
use russh_sftp::client::fs::{File, Metadata};
use russh_sftp::protocol::{FileType, OpenFlags};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::runtime::Handle;

use crate::connect::Session;
use crate::errors::{from_io, from_sftp};

const READ_CHUNK: usize = 256 * 1024;
const SEPARATOR: char = '/';
const HIDDEN_PREFIX: u8 = b'.';
const PARTIAL_SUFFIX: &str = "findermaxxing-partial";
const MILLIS_PER_SECOND: i64 = 1000;
const DOT_ENTRIES: [&str; 2] = [".", ".."];

static PARTIALS: AtomicU64 = AtomicU64::new(0);

/// One account as a mount. Cheap to clone; clones share the session.
#[derive(Clone)]
pub struct SftpFs {
    session: Arc<Session>,
    /// The account's root folder on the server, absolute, without a trailing `/`.
    root_path: String,
    root: StoragePath,
    runtime: Handle,
}

impl std::fmt::Debug for SftpFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SftpFs").field("root", &self.root_path).finish_non_exhaustive()
    }
}

impl SftpFs {
    pub(crate) fn new(session: Arc<Session>, root_path: String, root: StoragePath, runtime: Handle) -> Self {
        Self { session, root_path, root, runtime }
    }

    /// SFTP names are strings: a name that isn't UTF-8 can't be addressed.
    fn remote(&self, location: &StoragePath) -> Result<String, VfsError> {
        let mut path = self.root_path.clone();
        for segment in location.segments() {
            let name = std::str::from_utf8(segment.as_bytes())
                .map_err(|_| VfsError::InvalidPath(PathError::InvalidSegment(segment.display_name().into_owned())))?;
            path.push(SEPARATOR);
            path.push_str(name);
        }
        if path.is_empty() {
            path.push(SEPARATOR);
        }
        Ok(path)
    }

    async fn lstat(&self, location: &StoragePath) -> Result<Metadata, VfsError> {
        self.session.sftp.symlink_metadata(self.remote(location)?).await.map_err(|e| from_sftp(&e))
    }

    async fn exists(&self, location: &StoragePath) -> Result<bool, VfsError> {
        match self.lstat(location).await {
            Ok(_) => Ok(true),
            Err(VfsError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn require_folder(&self, location: &StoragePath) -> Result<(), VfsError> {
        let path = self.remote(location)?;
        match self.session.sftp.metadata(path).await.map_err(|e| from_sftp(&e))? {
            metadata if metadata.is_dir() => Ok(()),
            _ => Err(VfsError::NotADirectory),
        }
    }

    async fn kind(&self, path: &str, metadata: &Metadata) -> EntryKind {
        match metadata.file_type() {
            FileType::Dir => EntryKind::Directory,
            FileType::File => EntryKind::File,
            FileType::Symlink => EntryKind::Symlink(match self.session.sftp.metadata(path).await {
                Ok(target) if target.is_dir() => SymlinkTarget::Directory,
                Ok(_) => SymlinkTarget::File,
                Err(_) => SymlinkTarget::Broken,
            }),
            FileType::Other => EntryKind::Other,
        }
    }

    async fn entry(&self, parent: &StoragePath, name: PathSegment, metadata: &Metadata) -> Result<FileEntry, VfsError> {
        let location = parent.join(name.clone());
        let kind = self.kind(&self.remote(&location)?, metadata).await;
        let hidden = name.as_bytes().first() == Some(&HIDDEN_PREFIX);
        let metadata = EntryMetadata {
            size: (kind == EntryKind::File).then(|| ByteSize(metadata.size.unwrap_or_default())),
            modified: metadata.mtime.map(|seconds| Timestamp::from_unix_millis(i64::from(seconds) * MILLIS_PER_SECOND)),
            visibility: if hidden { Visibility::Hidden } else { Visibility::Visible },
            permissions: metadata.permissions.map(UnixMode::from_mode),
        };
        Ok(FileEntry::new(parent, name, kind, metadata))
    }

    async fn children(&self, dir: &StoragePath) -> Result<Vec<FileEntry>, VfsError> {
        let path = self.remote(dir)?;
        let listing = match self.session.sftp.read_dir(path).await {
            Ok(listing) => listing,
            Err(error) => {
                let error = from_sftp(&error);
                return Err(match self.lstat(dir).await {
                    Ok(metadata) if !metadata.is_dir() => VfsError::NotADirectory,
                    Ok(_) => error,
                    Err(stat_error) => stat_error,
                });
            }
        };
        let mut entries = Vec::new();
        for item in listing {
            let name = item.file_name();
            if DOT_ENTRIES.contains(&name.as_str()) {
                continue;
            }
            let segment = PathSegment::new(&name)?;
            entries.push(self.entry(dir, segment, &item.metadata()).await?);
        }
        Ok(entries)
    }

    async fn remove_tree(&self, location: &StoragePath, cancel: &CancelToken) -> Result<(), VfsError> {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let path = self.remote(location)?;
        if self.lstat(location).await?.is_dir() {
            for child in self.children(location).await? {
                Box::pin(self.remove_tree(child.path(), cancel)).await?;
            }
            self.session.sftp.remove_dir(path).await.map_err(|e| from_sftp(&e))
        } else {
            self.session.sftp.remove_file(path).await.map_err(|e| from_sftp(&e))
        }
    }

    fn partial_for(&self, target: &StoragePath, state: &ResumeState) -> Result<StoragePath, VfsError> {
        let parent = target.parent().ok_or(VfsError::NotFound)?;
        let ours = state.0.starts_with(char::from(HIDDEN_PREFIX)) && state.0.contains(PARTIAL_SUFFIX);
        if !ours || state.0.contains(SEPARATOR) {
            return Err(VfsError::NotFound);
        }
        Ok(parent.join(PathSegment::new(&state.0)?))
    }
}

impl Lister for SftpFs {
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
        let metadata = self.lstat(path).await?;
        self.entry(&parent, name, &metadata).await
    }
}

impl Reader for SftpFs {
    async fn read(&self, path: &StoragePath, cancel: CancelToken) -> Result<ByteStream, VfsError> {
        self.read_from(path, ByteSize(0), cancel).await
    }

    async fn read_from(
        &self,
        path: &StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> Result<ByteStream, VfsError> {
        let remote = self.remote(path)?;
        if self.session.sftp.metadata(remote.clone()).await.map_err(|e| from_sftp(&e))?.is_dir() {
            return Err(VfsError::IsADirectory);
        }
        let mut file = self.session.sftp.open(remote).await.map_err(|e| from_sftp(&e))?;
        if offset.0 > 0 {
            file.seek(std::io::SeekFrom::Start(offset.0)).await.map_err(|e| from_io(&e))?;
        }
        let chunks = stream::unfold(Some(file), move |file| {
            let cancel = cancel.clone();
            async move {
                let mut file = file?;
                if cancel.is_cancelled() {
                    return Some((Err(VfsError::Cancelled), None));
                }
                let mut buffer = vec![0; READ_CHUNK];
                match file.read(&mut buffer).await {
                    Ok(0) => None,
                    Ok(read) => {
                        buffer.truncate(read);
                        Some((Ok(buffer), Some(file)))
                    }
                    Err(error) => Some((Err(from_io(&error)), None)),
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

impl Writer for SftpFs {
    async fn create(&self, path: &StoragePath, _size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        if self.exists(path).await? {
            return Err(VfsError::AlreadyExists);
        }
        let parent = path.parent().ok_or(VfsError::NotFound)?;
        self.require_folder(&parent).await?;
        let partial = parent.join(partial_name(path)?);
        let flags = OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE;
        let file = self.session.sftp.open_with_flags(self.remote(&partial)?, flags).await.map_err(|e| from_sftp(&e))?;
        Ok(Box::new(SftpSink { fs: self.clone(), target: path.clone(), partial, file: Some(file), kept: false }))
    }

    async fn resume(&self, path: &StoragePath, state: &ResumeState) -> Result<ResumedSink, VfsError> {
        if self.exists(path).await? {
            return Err(VfsError::AlreadyExists);
        }
        let partial = self.partial_for(path, state)?;
        let size = self.lstat(&partial).await?.size.unwrap_or_default();
        let flags = OpenFlags::WRITE | OpenFlags::APPEND;
        let file = self.session.sftp.open_with_flags(self.remote(&partial)?, flags).await.map_err(|e| from_sftp(&e))?;
        let sink = SftpSink { fs: self.clone(), target: path.clone(), partial, file: Some(file), kept: true };
        Ok(ResumedSink { sink: Box::new(sink), offset: ByteSize(size) })
    }

    async fn discard(&self, path: &StoragePath, state: &ResumeState) -> Result<(), VfsError> {
        let partial = self.partial_for(path, state)?;
        match self.session.sftp.remove_file(self.remote(&partial)?).await.map_err(|e| from_sftp(&e)) {
            Ok(()) | Err(VfsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

struct SftpSink {
    fs: SftpFs,
    target: StoragePath,
    partial: StoragePath,
    /// `None` once committed or aborted.
    file: Option<File>,
    /// Checkpointed: the partial file outlives the sink for resuming.
    kept: bool,
}

impl SftpSink {
    fn remove_partial(&self) -> BoxFuture<'static, ()> {
        let (fs, partial) = (self.fs.clone(), self.partial.clone());
        async move {
            if let Ok(path) = fs.remote(&partial) {
                let _ = fs.session.sftp.remove_file(path).await;
            }
        }
        .boxed()
    }
}

impl WriteSink for SftpSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        async move {
            let file = self.file.as_mut().ok_or(VfsError::Conflict)?;
            file.write_all(&chunk).await.map_err(|e| from_io(&e))
        }
        .boxed()
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        async move {
            let file = self.file.take().ok_or(VfsError::Conflict)?;
            let result = async {
                file.sync_all().await.map_err(|e| from_sftp(&e))?;
                file.close().await.map_err(|e| from_io(&e))?;
                if self.fs.exists(&self.target).await? {
                    return Err(VfsError::AlreadyExists);
                }
                let (from, to) = (self.fs.remote(&self.partial)?, self.fs.remote(&self.target)?);
                self.fs.session.sftp.rename(from, to).await.map_err(|e| from_sftp(&e))
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
        self.file.take();
        self.kept = true;
        self.remove_partial()
    }

    fn checkpoint(&mut self) -> Option<ResumeState> {
        let name = std::str::from_utf8(self.partial.name()?.as_bytes()).ok()?.to_owned();
        self.kept = true;
        Some(ResumeState(name))
    }
}

impl Drop for SftpSink {
    fn drop(&mut self) {
        if !self.kept {
            self.file.take();
            self.fs.runtime.spawn(self.remove_partial());
        }
    }
}

impl Copier for SftpFs {
    /// SFTP v3 has no server-side copy: the bytes make a round trip through
    /// this machine, with the same no-overwrite, no-partial guarantees.
    async fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<ByteSize, VfsError> {
        if self.lstat(from).await?.is_dir() {
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

impl Mover for SftpFs {
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        self.lstat(from).await?;
        // A case-only rename names the same item on case-insensitive servers.
        let same_item = from.parent() == to.parent()
            && from.name().map(|n| n.display_name().to_lowercase())
                == to.name().map(|n| n.display_name().to_lowercase());
        if !same_item && self.exists(to).await? {
            return Err(VfsError::AlreadyExists);
        }
        let parent = to.parent().ok_or(VfsError::NotFound)?;
        self.require_folder(&parent).await?;
        self.session.sftp.rename(self.remote(from)?, self.remote(to)?).await.map_err(|e| from_sftp(&e))
    }
}

impl PermissionEditor for SftpFs {
    fn set_permissions<'a>(&'a self, path: &'a StoragePath, mode: UnixMode) -> BoxFuture<'a, Result<(), VfsError>> {
        async move {
            let mut attributes = Metadata::empty();
            attributes.permissions = Some(mode.bits());
            self.session.sftp.set_metadata(self.remote(path)?, attributes).await.map_err(|e| from_sftp(&e))
        }
        .boxed()
    }
}

impl Remover for SftpFs {
    async fn remove(&self, path: &StoragePath, cancel: CancelToken) -> Result<(), VfsError> {
        self.remove_tree(path, &cancel).await
    }
}

impl FolderMaker for SftpFs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        if self.exists(path).await? {
            return Err(VfsError::AlreadyExists);
        }
        let parent = path.parent().ok_or(VfsError::InvalidPath(PathError::Root))?;
        self.require_folder(&parent).await?;
        self.session.sftp.create_dir(self.remote(path)?).await.map_err(|e| from_sftp(&e))?;
        self.stat(path).await
    }
}

impl SftpFs {
    #[must_use]
    pub fn root(&self) -> &StoragePath {
        &self.root
    }
}
