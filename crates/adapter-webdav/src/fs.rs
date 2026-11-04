//! The WebDAV roles for one account. Writes go to a hidden partial file that
//! is MOVEd onto the target without overwriting on commit, so a half-written
//! file is never visible and nothing is ever replaced.

use std::sync::atomic::{AtomicU64, Ordering};

use application::CancelToken;
use application::ports::{
    ByteStream, Copier, FolderMaker, Lister, Mover, ProgressCallback, Reader, Remover, WriteSink, Writer,
};
use domain::{
    ByteSize, EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, PathSegment, StoragePath, VfsError,
    Visibility,
};
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use futures::{FutureExt, SinkExt, StreamExt};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use tokio::task::JoinHandle;

use crate::multistatus::{DavItem, parse};
use crate::urls::{Base, Kind};

const DEPTH: &str = "Depth";
const DESTINATION: &str = "Destination";
const OVERWRITE: &str = "Overwrite";
const NEVER_OVERWRITE: &str = "F";
const RANGE: &str = "Range";
const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:"><prop><resourcetype/><getcontentlength/><getlastmodified/></prop></propfind>"#;
const HIDDEN_PREFIX: u8 = b'.';
const PARTIAL_SUFFIX: &str = "findermaxxing-partial";
/// Chunks waiting to be sent per upload.
const UPLOAD_QUEUE: usize = 4;

static PARTIALS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
enum Depth {
    Item,
    Children,
}

impl Depth {
    fn header(self) -> &'static str {
        match self {
            Self::Item => "0",
            Self::Children => "1",
        }
    }
}

/// An HTTP client bound to one account's root and credentials.
#[derive(Clone)]
pub(crate) struct Connection {
    pub http: reqwest::Client,
    pub base: Base,
    pub username: String,
    pub password: Option<String>,
}

impl Connection {
    pub(crate) fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let request = self.http.request(method, url);
        if self.username.is_empty() { request } else { request.basic_auth(&self.username, self.password.as_ref()) }
    }

    async fn send(&self, request: RequestBuilder) -> Result<Response, VfsError> {
        request.send().await.map_err(|error| network_error(&error))
    }

    /// PROPFIND; `Ok(None)` for 404.
    async fn propfind(&self, url: &str, depth: Depth) -> Result<Vec<DavItem>, VfsError> {
        let method = Method::from_bytes(b"PROPFIND").map_err(|e| VfsError::Io { message: e.to_string() })?;
        let request = self.request(method, url).header(DEPTH, depth.header()).body(PROPFIND_BODY);
        let response = check(self.send(request).await?)?;
        let xml = response.text().await.map_err(|error| network_error(&error))?;
        parse(&xml).map_err(|message| VfsError::Io { message })
    }

    pub(crate) async fn check_root(&self, root: &StoragePath) -> Result<(), VfsError> {
        self.propfind(&self.base.url(root, Kind::Folder), Depth::Item).await.map(drop)
    }
}

pub(crate) fn network_error(error: &reqwest::Error) -> VfsError {
    if error.is_timeout() || error.is_connect() {
        VfsError::Transient { retry_after: None }
    } else {
        VfsError::Io { message: error.to_string() }
    }
}

/// Status codes → the shared contract. Callers map method-specific codes
/// (405 on MKCOL, 412 with `Overwrite: F`) before falling back to this.
fn status_error(status: StatusCode) -> VfsError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => VfsError::PermissionDenied,
        StatusCode::NOT_FOUND | StatusCode::GONE | StatusCode::CONFLICT => VfsError::NotFound,
        StatusCode::PRECONDITION_FAILED => VfsError::AlreadyExists,
        StatusCode::LOCKED => VfsError::Conflict,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            VfsError::Transient { retry_after: None }
        }
        status => VfsError::Io { message: format!("the server answered {status}") },
    }
}

fn check(response: Response) -> Result<Response, VfsError> {
    if response.status().is_success() { Ok(response) } else { Err(status_error(response.status())) }
}

/// One account as a mount.
#[derive(Clone)]
pub struct DavFs {
    connection: Connection,
    root: StoragePath,
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for DavFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavFs").field("root", &self.root).finish_non_exhaustive()
    }
}

impl DavFs {
    pub(crate) fn new(connection: Connection, root: StoragePath, runtime: tokio::runtime::Handle) -> Self {
        Self { connection, root, runtime }
    }

    fn url(&self, location: &StoragePath, kind: Kind) -> String {
        self.connection.base.url(location, kind)
    }

    fn entry(&self, item: &DavItem) -> Option<FileEntry> {
        let location = self.connection.base.location(&self.root, &item.href)?;
        let (parent, name) = (location.parent()?, location.name()?.clone());
        let kind = if item.is_collection { EntryKind::Directory } else { EntryKind::File };
        let hidden = name.as_bytes().first() == Some(&HIDDEN_PREFIX);
        let metadata = EntryMetadata {
            size: (kind == EntryKind::File).then(|| ByteSize(item.size.unwrap_or_default())),
            modified: item.modified,
            visibility: if hidden { Visibility::Hidden } else { Visibility::Visible },
            permissions: None,
        };
        Some(FileEntry::new(&parent, name, kind, metadata))
    }

    async fn kind_of(&self, path: &StoragePath) -> Result<Kind, VfsError> {
        if path.parent().is_none() {
            return Ok(Kind::Folder);
        }
        let entry = self.stat(path).await?;
        Ok(if entry.kind() == EntryKind::Directory { Kind::Folder } else { Kind::File })
    }

    /// Why a folder listing failed: servers answer a folder URL that is
    /// really a file with 400, 404 or 409, so look at the item itself.
    async fn listing_error(&self, dir: &StoragePath, error: VfsError) -> VfsError {
        if matches!(error, VfsError::PermissionDenied | VfsError::Transient { .. }) {
            return error;
        }
        match self.stat(dir).await {
            Ok(entry) if entry.kind() != EntryKind::Directory => VfsError::NotADirectory,
            Ok(_) => error,
            Err(stat_error) => stat_error,
        }
    }

    async fn transfer(&self, method: &[u8], from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        let kind = self.kind_of(from).await?;
        let method = Method::from_bytes(method).map_err(|e| VfsError::Io { message: e.to_string() })?;
        let request = self
            .connection
            .request(method, &self.url(from, kind))
            .header(DESTINATION, self.url(to, kind))
            .header(OVERWRITE, NEVER_OVERWRITE)
            .header(DEPTH, "infinity");
        check(self.connection.send(request).await?).map(drop)
    }

    async fn delete_url(&self, url: &str) -> Result<(), VfsError> {
        check(self.connection.send(self.connection.request(Method::DELETE, url)).await?).map(drop)
    }
}

impl Lister for DavFs {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let (fs, dir) = (self.clone(), dir.clone());
        stream::once(async move {
            if cancel.is_cancelled() {
                return stream::iter(vec![Err(VfsError::Cancelled)]);
            }
            let items = match fs.connection.propfind(&fs.url(&dir, Kind::Folder), Depth::Children).await {
                Ok(items) => items,
                Err(error) => return stream::iter(vec![Err(fs.listing_error(&dir, error).await)]),
            };
            let mut children = Vec::new();
            let mut is_folder = false;
            for item in &items {
                match fs.connection.base.location(&fs.root, &item.href) {
                    Some(location) if location == dir => is_folder = item.is_collection,
                    Some(location) if location.is_child_of(&dir) => children.extend(fs.entry(item)),
                    _ => {}
                }
            }
            if !is_folder {
                return stream::iter(vec![Err(VfsError::NotADirectory)]);
            }
            let mut batches: Vec<Result<EntryBatch, VfsError>> =
                children.chunks(LIST_BATCH_SIZE).map(|chunk| Ok(chunk.to_vec())).collect();
            if cancel.is_cancelled() {
                batches = vec![Err(VfsError::Cancelled)];
            }
            stream::iter(batches)
        })
        .flatten()
    }

    async fn stat(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        let items = self.connection.propfind(&self.url(path, Kind::File), Depth::Item).await?;
        let item = items.first().ok_or(VfsError::NotFound)?;
        if path.parent().is_none() {
            return Err(VfsError::InvalidPath(domain::PathError::Root));
        }
        self.entry(&DavItem { href: self.url(path, Kind::File), ..item.clone() }).ok_or(VfsError::NotFound)
    }
}

impl Reader for DavFs {
    async fn read(&self, path: &StoragePath, cancel: CancelToken) -> Result<ByteStream, VfsError> {
        self.read_from(path, ByteSize(0), cancel).await
    }

    async fn read_from(
        &self,
        path: &StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> Result<ByteStream, VfsError> {
        // Many servers answer GET on a folder with an HTML index page.
        if self.kind_of(path).await? == Kind::Folder {
            return Err(VfsError::IsADirectory);
        }
        let mut request = self.connection.request(Method::GET, &self.url(path, Kind::File));
        if offset.0 > 0 {
            request = request.header(RANGE, format!("bytes={}-", offset.0));
        }
        let response = self.connection.send(request).await?;
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(stream::empty().boxed());
        }
        if offset.0 > 0 && response.status() == StatusCode::OK {
            // The server ignored the range: skip the bytes ourselves.
            let body = body_stream(check(response)?, cancel);
            return Ok(application::ports::skip_bytes(body, offset));
        }
        Ok(body_stream(check(response)?, cancel))
    }
}

fn body_stream(response: Response, cancel: CancelToken) -> ByteStream {
    response
        .bytes_stream()
        .map(move |chunk| {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            chunk.map(|bytes| bytes.to_vec()).map_err(|error| network_error(&error))
        })
        .boxed()
}

impl Writer for DavFs {
    async fn create(&self, path: &StoragePath, _size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        match self.stat(path).await {
            Ok(_) => return Err(VfsError::AlreadyExists),
            Err(VfsError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let parent = path.parent().ok_or(VfsError::NotFound)?;
        if self.kind_of(&parent).await? != Kind::Folder {
            return Err(VfsError::NotADirectory);
        }
        let partial = parent.join(partial_name(path)?);
        let (sender, receiver) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(UPLOAD_QUEUE);
        let request = self
            .connection
            .request(Method::PUT, &self.url(&partial, Kind::File))
            .body(reqwest::Body::wrap_stream(receiver));
        let upload = self.runtime.spawn(async move { request.send().await });
        Ok(Box::new(DavSink {
            fs: self.clone(),
            target: path.clone(),
            partial,
            sender: Some(sender),
            upload: Some(upload),
        }))
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

struct DavSink {
    fs: DavFs,
    target: StoragePath,
    partial: StoragePath,
    /// Closed (taken) when the upload is finished or abandoned.
    sender: Option<mpsc::Sender<Result<Vec<u8>, std::io::Error>>>,
    upload: Option<JoinHandle<reqwest::Result<Response>>>,
}

impl DavSink {
    async fn upload_result(&mut self) -> Result<(), VfsError> {
        self.sender.take();
        let upload = self.upload.take().ok_or(VfsError::Conflict)?;
        let response = upload.await.map_err(|join| VfsError::Io { message: join.to_string() })?;
        check(response.map_err(|error| network_error(&error))?).map(drop)
    }

    fn remove_partial(&self) -> BoxFuture<'static, ()> {
        let (fs, url) = (self.fs.clone(), self.fs.url(&self.partial, Kind::File));
        async move {
            let _ = fs.delete_url(&url).await;
        }
        .boxed()
    }
}

impl WriteSink for DavSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        async move {
            let sender = self.sender.as_mut().ok_or(VfsError::Conflict)?;
            if sender.send(Ok(chunk)).await.is_err() {
                // The request ended early; its response says why.
                self.upload_result().await?;
                return Err(VfsError::Io { message: "the upload ended early".into() });
            }
            Ok(())
        }
        .boxed()
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        async move {
            let result = match self.upload_result().await {
                Ok(()) => self.fs.transfer(b"MOVE", &self.partial, &self.target).await,
                Err(error) => Err(error),
            };
            if result.is_err() {
                self.remove_partial().await;
            }
            result
        }
        .boxed()
    }

    fn abort(mut self: Box<Self>) -> BoxFuture<'static, ()> {
        if let Some(mut sender) = self.sender.take() {
            let _ = sender.try_send(Err(std::io::Error::other("upload aborted")));
        }
        if let Some(upload) = self.upload.take() {
            upload.abort();
        }
        self.remove_partial()
    }
}

impl Drop for DavSink {
    fn drop(&mut self) {
        if self.upload.is_some() {
            if let Some(upload) = self.upload.take() {
                upload.abort();
            }
            self.fs.runtime.spawn(self.remove_partial());
        }
    }
}

impl Copier for DavFs {
    /// Server-side `COPY`; nothing passes through this machine.
    async fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<ByteSize, VfsError> {
        let source = self.stat(from).await?;
        if source.kind() == EntryKind::Directory {
            return Err(VfsError::IsADirectory);
        }
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        self.transfer(b"COPY", from, to).await?;
        let size = source.metadata().size.unwrap_or_default();
        progress(size);
        Ok(size)
    }
}

impl Mover for DavFs {
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        self.transfer(b"MOVE", from, to).await
    }
}

impl Remover for DavFs {
    async fn remove(&self, path: &StoragePath, cancel: CancelToken) -> Result<(), VfsError> {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let kind = self.kind_of(path).await?;
        self.delete_url(&self.url(path, kind)).await
    }
}

impl FolderMaker for DavFs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        let method = Method::from_bytes(b"MKCOL").map_err(|e| VfsError::Io { message: e.to_string() })?;
        let response = self.connection.send(self.connection.request(method, &self.url(path, Kind::Folder))).await?;
        match response.status() {
            StatusCode::METHOD_NOT_ALLOWED => Err(VfsError::AlreadyExists),
            _ => check(response).map(drop),
        }?;
        self.stat(path).await
    }
}
