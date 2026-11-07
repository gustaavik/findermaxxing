//! Compressing items into an archive and extracting one, on any provider:
//! archives are read as local copies, and everything written goes through
//! the destination's `Writer`/`FolderMaker`. Both are undoable (the new
//! archive or folder goes to the Trash).

use std::collections::HashSet;
use std::sync::Arc;

use domain::{
    ArchiveFormat, ByteSize, EntryKind, Operation, OperationKind, PathSegment, StoragePath, UnixMode, VfsError,
    archive_name, extraction_folder_name,
};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt, TryStreamExt};

use super::checksums::opaque_debug;
use super::support::{create_free_folder, free_path, local_copy, no_progress, pump};
use crate::cancel::CancelToken;
use crate::operations::progress::ProgressTracker;
use crate::operations::scan::{ScannedTree, scan};
use crate::operations::{JobContext, OperationError, OperationHistory, OperationReport};
use crate::ports::{
    ArchiveBuilder, ArchiveError, ArchiveEvent, ArchiveExtractor, MountHandle, MountResolver, PermissionEditor,
    WriteSink,
};

/// Contents waiting to be packed; small, so reading never runs far ahead.
const PACKING_QUEUE: usize = 8;
/// Any of these bits means "executable": worth keeping when extracting.
const EXECUTE_BITS: u32 = 0o111;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractRequest {
    pub archive: StoragePath,
    /// The new folder is created in here.
    pub destination: StoragePath,
}

pub struct ExtractArchive {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    extractor: Arc<dyn ArchiveExtractor>,
}

impl ExtractArchive {
    #[must_use]
    pub fn new(
        mounts: Arc<dyn MountResolver>,
        history: Arc<OperationHistory>,
        extractor: Arc<dyn ArchiveExtractor>,
    ) -> Self {
        Self { mounts, history, extractor }
    }

    /// Extracts into a new folder named after the archive (`photos 2` if
    /// `photos` exists). What was extracted stays if it fails half-way.
    pub async fn execute(&self, request: &ExtractRequest, ctx: &JobContext) -> OperationReport {
        let archive = &request.archive;
        let Some(name) = archive.name() else {
            return OperationReport::new(
                Operation::sequence(Vec::new()),
                Err(OperationError::WrongKind(archive.clone())),
            );
        };
        let Some(format) = ArchiveFormat::of(name) else {
            return OperationReport::new(
                Operation::sequence(Vec::new()),
                Err(OperationError::WrongKind(archive.clone())),
            );
        };
        let prepared = async {
            let local = local_copy(&*self.mounts, archive).await?;
            let mount = self.mounts.resolve(request.destination.mount())?;
            let root = create_free_folder(&mount, &request.destination, &extraction_folder_name(name)?).await?;
            Ok::<_, VfsError>((local, mount, root))
        };
        let (local, mount, root) = match prepared.await {
            Ok(prepared) => prepared,
            Err(error) => {
                return OperationReport::new(Operation::sequence(Vec::new()), Err(OperationError::on(archive)(error)));
            }
        };
        let done = Operation::Created(vec![root.clone()]);
        let events = self.extractor.extract(local, format, ctx.cancel.clone());
        let mut unpacker = Unpacker::new(&mount, root, ProgressTracker::new(Arc::clone(&ctx.observer)), &ctx.cancel);
        let result = unpacker.run(events).await;
        self.history.record(OperationKind::Extract, done.clone());
        OperationReport::new(done, result)
    }
}

/// Writes extracted events below `root`.
struct Unpacker<'a> {
    mount: &'a MountHandle,
    root: StoragePath,
    tracker: Arc<ProgressTracker>,
    cancel: &'a CancelToken,
    folders: HashSet<Vec<PathSegment>>,
    permissions: Option<Arc<dyn PermissionEditor>>,
    skipped: usize,
}

/// The file being written.
struct OpenFile {
    sink: Box<dyn WriteSink>,
    path: StoragePath,
    mode: Option<UnixMode>,
}

impl<'a> Unpacker<'a> {
    fn new(mount: &'a MountHandle, root: StoragePath, tracker: Arc<ProgressTracker>, cancel: &'a CancelToken) -> Self {
        let permissions = mount.permission_editor().ok();
        Self { mount, root, tracker, cancel, folders: HashSet::new(), permissions, skipped: 0 }
    }

    async fn run(
        &mut self,
        mut events: futures::stream::BoxStream<'static, Result<ArchiveEvent, ArchiveError>>,
    ) -> Result<(), OperationError> {
        let mut open: Option<OpenFile> = None;
        let result = async {
            while let Some(event) = events.next().await {
                if self.cancel.is_cancelled() {
                    return Err(OperationError::Cancelled);
                }
                self.handle(event.map_err(archive_error)?, &mut open).await?;
            }
            Ok(())
        }
        .await;
        if let Some(file) = open.take() {
            file.sink.abort().await;
        }
        result?;
        if self.skipped > 0 { Err(OperationError::SkippedEntries { count: self.skipped }) } else { Ok(()) }
    }

    async fn handle(&mut self, event: ArchiveEvent, open: &mut Option<OpenFile>) -> Result<(), OperationError> {
        match event {
            ArchiveEvent::Folder(path) => self.ensure_folder(&path).await,
            ArchiveEvent::File { path, size, mode, .. } => {
                let Some((name, parent)) = path.split_last() else { return Ok(()) };
                self.ensure_folder(parent).await?;
                self.tracker.item_started(Some(name));
                let target = below(&self.root, &path);
                let writer = self.mount.writer().map_err(OperationError::on(&target))?;
                match writer.create_boxed(&target, size).await {
                    Ok(sink) => *open = Some(OpenFile { sink, path: target, mode }),
                    // The same path twice in one archive: keep the first.
                    Err(VfsError::AlreadyExists) => self.skipped += 1,
                    Err(error) => return Err(OperationError::on(&target)(error)),
                }
                Ok(())
            }
            ArchiveEvent::Chunk(bytes) => {
                if let Some(file) = open.as_mut() {
                    let length = bytes.len() as u64;
                    file.sink.write(bytes).await.map_err(OperationError::on(&file.path))?;
                    (self.tracker.byte_callback())(ByteSize(length));
                }
                Ok(())
            }
            ArchiveEvent::EndFile => match open.take() {
                Some(file) => self.finish(file).await,
                None => Ok(()),
            },
            ArchiveEvent::Skipped { .. } => {
                self.skipped += 1;
                Ok(())
            }
        }
    }

    async fn finish(&mut self, file: OpenFile) -> Result<(), OperationError> {
        file.sink.commit().await.map_err(OperationError::on(&file.path))?;
        if let (Some(mode), Some(editor)) = (file.mode, &self.permissions)
            && mode.bits() & EXECUTE_BITS != 0
        {
            // Scripts and binaries stay runnable; failing that isn't worth stopping for.
            let _ = editor.set_permissions(&file.path, mode).await;
        }
        self.tracker.item_finished();
        Ok(())
    }

    /// Creates `path` and its parents below the root, once each.
    async fn ensure_folder(&mut self, path: &[PathSegment]) -> Result<(), OperationError> {
        for depth in 1..=path.len() {
            let prefix = &path[..depth];
            if self.folders.contains(prefix) {
                continue;
            }
            let folder = below(&self.root, prefix);
            let maker = self.mount.folder_maker().map_err(OperationError::on(&folder))?;
            match maker.create_folder_boxed(&folder).await {
                Ok(_) | Err(VfsError::AlreadyExists) => {}
                Err(error) => return Err(OperationError::on(&folder)(error)),
            }
            self.folders.insert(prefix.to_vec());
        }
        Ok(())
    }
}

fn below(root: &StoragePath, relative: &[PathSegment]) -> StoragePath {
    relative.iter().fold(root.clone(), |path, segment| path.join(segment.clone()))
}

fn archive_error(error: ArchiveError) -> OperationError {
    match error {
        ArchiveError::Io(VfsError::Cancelled) => OperationError::Cancelled,
        other => OperationError::Archive(other),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressRequest {
    pub items: Vec<StoragePath>,
    pub format: ArchiveFormat,
    /// The archive is created in here.
    pub destination: StoragePath,
}

pub struct CompressItems {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    builder: Arc<dyn ArchiveBuilder>,
}

impl CompressItems {
    #[must_use]
    pub fn new(
        mounts: Arc<dyn MountResolver>,
        history: Arc<OperationHistory>,
        builder: Arc<dyn ArchiveBuilder>,
    ) -> Self {
        Self { mounts, history, builder }
    }

    /// Packs the items (folders with everything inside; links are left
    /// out) into `Name.zip` / `Archive.zip`, numbered if the name is taken.
    pub async fn execute(&self, request: &CompressRequest, ctx: &JobContext) -> OperationReport {
        let nothing = || Operation::sequence(Vec::new());
        if !request.format.can_create() {
            return OperationReport::new(
                nothing(),
                Err(OperationError::Archive(ArchiveError::CannotCreate(request.format))),
            );
        }
        let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
        let trees = match self.scan_all(&request.items, &ctx.cancel).await {
            Ok(trees) => trees,
            Err(error) => return OperationReport::new(nothing(), Err(error)),
        };
        tracker.begin(trees.iter().map(ScannedTree::item_count).sum(), ByteSize(trees.iter().map(|t| t.bytes.0).sum()));
        match self.write_archive(request, trees, &tracker, &ctx.cancel).await {
            Ok(path) => {
                let done = Operation::Created(vec![path]);
                self.history.record(OperationKind::Compress, done.clone());
                OperationReport::new(done, Ok(()))
            }
            Err(error) => OperationReport::new(nothing(), Err(error)),
        }
    }

    async fn scan_all(&self, items: &[StoragePath], cancel: &CancelToken) -> Result<Vec<ScannedTree>, OperationError> {
        let mut trees = Vec::with_capacity(items.len());
        for item in items {
            let tree = async {
                let lister = self.mounts.resolve(item.mount())?.lister()?;
                let root = lister.stat_boxed(item).await?;
                scan(&*lister, root, cancel).await
            };
            trees.push(tree.await.map_err(OperationError::on(item))?);
        }
        Ok(trees)
    }

    async fn write_archive(
        &self,
        request: &CompressRequest,
        trees: Vec<ScannedTree>,
        tracker: &Arc<ProgressTracker>,
        cancel: &CancelToken,
    ) -> Result<StoragePath, OperationError> {
        let destination = &request.destination;
        let mount = self.mounts.resolve(destination.mount()).map_err(OperationError::on(destination))?;
        let name =
            archive_name(&request.items, request.format).map_err(|e| OperationError::on(destination)(e.into()))?;
        let path =
            free_path(&mount, destination, &name, EntryKind::File).await.map_err(OperationError::on(destination))?;
        let (contents, queue) = mpsc::channel(PACKING_QUEUE);
        let packed = self.builder.build(request.format, queue.boxed(), cancel.clone());
        let feeding = feed(&*self.mounts, trees, contents, tracker, cancel);
        let writing = async {
            let mut sink = mount.writer()?.create_boxed(&path, None).await?;
            let chunks = packed.map_err(|error| match error {
                ArchiveError::Io(error) => error,
                other => VfsError::Io { message: other.to_string() },
            });
            match pump(&mut *sink, chunks.boxed(), &no_progress(), cancel).await {
                Ok(()) => Ok(sink),
                Err(error) => {
                    sink.abort().await;
                    Err(error)
                }
            }
        };
        let (fed, written) = futures::join!(feeding, writing);
        let sink = written.map_err(OperationError::on(&path))?;
        if let Err(error) = fed {
            // The builder saw the contents end early: its archive is incomplete.
            sink.abort().await;
            return Err(error);
        }
        sink.commit().await.map_err(OperationError::on(&path))?;
        Ok(path)
    }
}

/// Sends every scanned item to the archive builder: folders, then files as
/// their chunks arrive. Links and special files are left out.
async fn feed(
    mounts: &dyn MountResolver,
    trees: Vec<ScannedTree>,
    mut contents: mpsc::Sender<ArchiveEvent>,
    tracker: &Arc<ProgressTracker>,
    cancel: &CancelToken,
) -> Result<(), OperationError> {
    let progress = tracker.byte_callback();
    // The builder only stops early when it failed; it reports that itself.
    let send_failed = |_| OperationError::Cancelled;
    for tree in trees {
        let base = tree.root.path().segments().len().saturating_sub(1);
        for item in &tree.items {
            if cancel.is_cancelled() {
                return Err(OperationError::Cancelled);
            }
            let relative = item.path().segments()[base..].to_vec();
            tracker.item_started(item.path().name());
            match item.kind() {
                EntryKind::Directory => contents.send(ArchiveEvent::Folder(relative)).await.map_err(send_failed)?,
                EntryKind::File => {
                    let size = item.metadata().size;
                    let mode = item.metadata().permissions;
                    let modified = item.metadata().modified;
                    let file = ArchiveEvent::File { path: relative, size, mode, modified };
                    contents.send(file).await.map_err(send_failed)?;
                    let reader = mounts
                        .resolve(item.path().mount())
                        .and_then(|m| m.reader())
                        .map_err(OperationError::on(item.path()))?;
                    let mut chunks = reader
                        .read_boxed(item.path(), cancel.clone())
                        .await
                        .map_err(OperationError::on(item.path()))?;
                    while let Some(chunk) = chunks.try_next().await.map_err(OperationError::on(item.path()))? {
                        let length = chunk.len() as u64;
                        contents.send(ArchiveEvent::Chunk(chunk)).await.map_err(send_failed)?;
                        progress(ByteSize(length));
                    }
                    contents.send(ArchiveEvent::EndFile).await.map_err(send_failed)?;
                }
                _ => {}
            }
            tracker.item_finished();
        }
    }
    Ok(())
}

opaque_debug!(ExtractArchive, CompressItems);
