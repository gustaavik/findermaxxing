//! The Google Drive roles for one account. Paths are resolved folder by
//! folder through (briefly cached) listings, so the disambiguated names the
//! app shows always lead to the right item.

use std::sync::{Arc, Mutex, PoisonError};

use application::CancelToken;
use application::ports::{
    ByteStream, Copier, FolderMaker, Lister, Mover, ProgressCallback, Reader, Remover, ResumeState, ResumedSink,
    Trasher, WriteSink, Writer, skip_bytes,
};
use domain::{
    ByteSize, Capability, EntryBatch, FileEntry, LIST_BATCH_SIZE, PathError, PathSegment, StoragePath, TrashTicket,
    VfsError,
};
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use futures::{FutureExt, StreamExt};
use infra_oauth::{ApiClient, network_error};
use reqwest::{Response, StatusCode};
use serde_json::{Value, json};
use tokio::runtime::Handle;

use crate::listing::{Child, Content, FOLDER_MIME, ListingCache, children_of, find};

pub(crate) const ROOT_ID: &str = "root";
const FIELDS: &str = "nextPageToken,files(id,name,mimeType,size,modifiedTime,createdTime)";
const PAGE_SIZE: &str = "1000";
/// Upload chunks: a multiple of Drive's required 256 KiB.
const CHUNK: usize = 8 * 1024 * 1024;
/// Where trashed items are said to be; never listed, only used in tickets.
const TRASH_SEGMENT: &str = ".trash";
const UPLOAD_LENGTH: &str = "X-Upload-Content-Length";
const CONTENT_RANGE: &str = "Content-Range";
const RANGE: &str = "Range";

/// One account as a mount. Clones share the client and listing cache.
#[derive(Clone)]
pub struct DriveFs {
    api: ApiClient,
    base: String,
    root: StoragePath,
    cache: Arc<Mutex<ListingCache>>,
    runtime: Handle,
}

impl std::fmt::Debug for DriveFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriveFs").field("root", &self.root).finish_non_exhaustive()
    }
}

/// An item found by path: the root, or a child of a folder.
struct Located {
    id: String,
    parent_id: Option<String>,
    child: Option<Child>,
}

impl Located {
    fn is_folder(&self) -> bool {
        self.child.as_ref().is_none_or(|c| c.content == Content::Folder)
    }
}

/// Drive's error statuses → the shared contract.
fn status_error(status: StatusCode, body: &str) -> VfsError {
    let reason = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["errors"][0]["reason"].as_str().map(str::to_owned))
        .unwrap_or_default();
    match (status, reason.as_str()) {
        (StatusCode::NOT_FOUND | StatusCode::GONE, _) => VfsError::NotFound,
        (StatusCode::FORBIDDEN, "rateLimitExceeded" | "userRateLimitExceeded") => {
            VfsError::Transient { retry_after: None }
        }
        (StatusCode::FORBIDDEN, "fileNotDownloadable" | "cannotCopyFolder") => VfsError::Unsupported(Capability::Read),
        (StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN, _) => VfsError::PermissionDenied,
        (status, _) if status.is_server_error() => VfsError::Transient { retry_after: None },
        (status, _) => VfsError::Io { message: format!("Google Drive answered {status} {reason}") },
    }
}

async fn checked(response: Response) -> Result<Response, VfsError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(status_error(status, &body))
}

async fn json(response: Response) -> Result<Value, VfsError> {
    let text = checked(response).await?.text().await.map_err(|e| network_error(&e))?;
    serde_json::from_str(&text).map_err(|e| VfsError::Io { message: e.to_string() })
}

impl DriveFs {
    pub(crate) fn new(api: ApiClient, base: String, root: StoragePath, runtime: Handle) -> Self {
        Self { api, base, root, cache: Arc::default(), runtime }
    }

    #[must_use]
    pub fn root(&self) -> &StoragePath {
        &self.root
    }

    fn files_url(&self) -> String {
        format!("{}/drive/v3/files", self.base)
    }

    fn file_url(&self, id: &str) -> String {
        format!("{}/drive/v3/files/{id}", self.base)
    }

    fn forget_listings(&self) {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    async fn children(&self, folder_id: &str) -> Result<Vec<Child>, VfsError> {
        if let Some(cached) = self.cache.lock().unwrap_or_else(PoisonError::into_inner).get(folder_id) {
            return Ok(cached);
        }
        let query = format!("'{folder_id}' in parents and trashed = false");
        let mut files = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let url = self.files_url();
            let mut params = vec![
                ("q", query.clone()),
                ("fields", FIELDS.to_owned()),
                ("pageSize", PAGE_SIZE.to_owned()),
                ("orderBy", "createdTime".to_owned()),
            ];
            if let Some(token) = &page_token {
                params.push(("pageToken", token.clone()));
            }
            let page = json(self.api.send(|http| http.get(&url).query(&params)).await?).await?;
            files.extend(page["files"].as_array().cloned().unwrap_or_default());
            match page["nextPageToken"].as_str() {
                Some(token) => page_token = Some(token.to_owned()),
                None => break,
            }
        }
        let children = children_of(&files);
        self.cache.lock().unwrap_or_else(PoisonError::into_inner).put(folder_id, children.clone());
        Ok(children)
    }

    async fn resolve(&self, path: &StoragePath) -> Result<Located, VfsError> {
        let mut located = Located { id: ROOT_ID.to_owned(), parent_id: None, child: None };
        for segment in path.segments() {
            if !located.is_folder() {
                return Err(VfsError::NotFound);
            }
            let children = self.children(&located.id).await?;
            let child = find(&children, segment)?.clone();
            located = Located { id: child.id.clone(), parent_id: Some(located.id), child: Some(child) };
        }
        Ok(located)
    }

    async fn folder_id(&self, path: &StoragePath) -> Result<String, VfsError> {
        let located = self.resolve(path).await?;
        if located.is_folder() { Ok(located.id) } else { Err(VfsError::NotADirectory) }
    }

    /// Always asks Drive: a stale listing could let a write create a
    /// duplicate (Drive allows them) or refuse a free name.
    async fn require_absent(&self, path: &StoragePath) -> Result<(), VfsError> {
        self.forget_listings();
        match self.resolve(path).await {
            Ok(_) => Err(VfsError::AlreadyExists),
            Err(VfsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The parent's id and the Drive name for a new item at `path`.
    async fn destination(&self, path: &StoragePath) -> Result<(String, String), VfsError> {
        let (parent, name) = path.parent().zip(path.name()).ok_or(VfsError::InvalidPath(PathError::Root))?;
        Ok((self.folder_id(&parent).await?, name.display_name().into_owned()))
    }

    async fn patch(&self, id: &str, query: Vec<(&'static str, String)>, body: Value) -> Result<Value, VfsError> {
        let url = self.file_url(id);
        let response = self.api.send(|http| http.patch(&url).query(&query).json(&body)).await?;
        self.forget_listings();
        json(response).await
    }
}

impl Lister for DriveFs {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let (fs, dir) = (self.clone(), dir.clone());
        stream::once(async move {
            let entries = async {
                let id = fs.folder_id(&dir).await?;
                Ok(fs.children(&id).await?.iter().map(|child| child.entry(&dir)).collect::<Vec<_>>())
            }
            .await;
            let batches: Vec<Result<EntryBatch, VfsError>> = match entries {
                Ok(_) if cancel.is_cancelled() => vec![Err(VfsError::Cancelled)],
                Ok(entries) => entries.chunks(LIST_BATCH_SIZE).map(|chunk| Ok(chunk.to_vec())).collect(),
                Err(error) => vec![Err(error)],
            };
            stream::iter(batches)
        })
        .flatten()
    }

    async fn stat(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        let parent = path.parent().ok_or(VfsError::InvalidPath(PathError::Root))?;
        let child = self.resolve(path).await?.child.ok_or(VfsError::InvalidPath(PathError::Root))?;
        Ok(child.entry(&parent))
    }
}

fn body_stream(response: Response, cancel: CancelToken) -> ByteStream {
    response
        .bytes_stream()
        .map(move |chunk| {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            chunk.map(|bytes| bytes.to_vec()).map_err(|e| network_error(&e))
        })
        .boxed()
}

impl Reader for DriveFs {
    async fn read(&self, path: &StoragePath, cancel: CancelToken) -> Result<ByteStream, VfsError> {
        self.read_from(path, ByteSize(0), cancel).await
    }

    async fn read_from(
        &self,
        path: &StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> Result<ByteStream, VfsError> {
        let child = self.resolve(path).await?.child.ok_or(VfsError::IsADirectory)?;
        match child.content {
            Content::Folder => Err(VfsError::IsADirectory),
            Content::Unreadable => Err(VfsError::Unsupported(Capability::Read)),
            Content::Export { mime, .. } => {
                let url = format!("{}/export", self.file_url(&child.id));
                let response =
                    checked(self.api.send(|http| http.get(&url).query(&[("mimeType", mime)])).await?).await?;
                Ok(skip_bytes(body_stream(response, cancel), offset))
            }
            Content::Binary => {
                let url = self.file_url(&child.id);
                let range = (offset.0 > 0).then(|| format!("bytes={}-", offset.0));
                let response = self
                    .api
                    .send(|http| {
                        let request = http.get(&url).query(&[("alt", "media")]);
                        match &range {
                            Some(range) => request.header(RANGE, range),
                            None => request,
                        }
                    })
                    .await?;
                if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
                    return Ok(stream::empty().boxed());
                }
                let ranged = response.status() == StatusCode::PARTIAL_CONTENT;
                let body = body_stream(checked(response).await?, cancel);
                Ok(if ranged || offset.0 == 0 { body } else { skip_bytes(body, offset) })
            }
        }
    }
}

impl Writer for DriveFs {
    /// A resumable upload session; the file appears only when the last
    /// chunk arrives.
    async fn create(&self, path: &StoragePath, size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        self.require_absent(path).await?;
        let (parent, name) = self.destination(path).await?;
        let url = format!("{}/upload/drive/v3/files", self.base);
        let metadata = json!({"name": name, "parents": [parent]});
        let response = self
            .api
            .send(|http| {
                let request = http.post(&url).query(&[("uploadType", "resumable")]).json(&metadata);
                match size_hint {
                    Some(size) => request.header(UPLOAD_LENGTH, size.0),
                    None => request,
                }
            })
            .await?;
        let response = checked(response).await?;
        let session = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| VfsError::Io { message: "no upload session".into() })?
            .to_owned();
        Ok(Box::new(UploadSink::new(self.clone(), session, 0, false)))
    }

    /// The state is the session URL; Drive says how much it has.
    async fn resume(&self, path: &StoragePath, state: &ResumeState) -> Result<ResumedSink, VfsError> {
        self.require_absent(path).await?;
        let session = state.0.clone();
        let response =
            self.api.send(|http| http.put(&session).header(CONTENT_RANGE, "bytes */*").body(Vec::new())).await?;
        if response.status() != StatusCode::PERMANENT_REDIRECT {
            return Err(if response.status().is_success() { VfsError::AlreadyExists } else { VfsError::NotFound });
        }
        let received = response
            .headers()
            .get(RANGE)
            .and_then(|v| v.to_str().ok()?.rsplit('-').next()?.parse::<u64>().ok())
            .map_or(0, |last| last + 1);
        let sink = UploadSink::new(self.clone(), session, received, true);
        Ok(ResumedSink { sink: Box::new(sink), offset: ByteSize(received) })
    }

    async fn discard(&self, _path: &StoragePath, state: &ResumeState) -> Result<(), VfsError> {
        let session = state.0.clone();
        self.api.send(|http| http.delete(&session)).await.map(drop)
    }
}

struct UploadSink {
    fs: DriveFs,
    session: String,
    buffer: Vec<u8>,
    sent: u64,
    kept: bool,
    finished: bool,
}

/// What Drive said to a chunk.
enum ChunkOutcome {
    MoreWanted,
    Complete,
}

impl UploadSink {
    fn new(fs: DriveFs, session: String, sent: u64, kept: bool) -> Self {
        Self { fs, session, buffer: Vec::new(), sent, kept, finished: false }
    }

    async fn put(&mut self, chunk: Vec<u8>, total: Option<u64>) -> Result<ChunkOutcome, VfsError> {
        let total_text = total.map_or_else(|| "*".to_owned(), |t| t.to_string());
        let range = if chunk.is_empty() {
            format!("bytes */{total_text}")
        } else {
            format!("bytes {}-{}/{total_text}", self.sent, self.sent + chunk.len() as u64 - 1)
        };
        let length = chunk.len() as u64;
        let session = self.session.clone();
        let response =
            self.fs.api.send(|http| http.put(&session).header(CONTENT_RANGE, &range).body(chunk.clone())).await?;
        match response.status() {
            StatusCode::PERMANENT_REDIRECT => {
                self.sent += length;
                Ok(ChunkOutcome::MoreWanted)
            }
            status if status.is_success() => {
                self.sent += length;
                Ok(ChunkOutcome::Complete)
            }
            _ => checked(response).await.map(|_| ChunkOutcome::MoreWanted),
        }
    }

    fn cancel_session(&self) -> BoxFuture<'static, ()> {
        let (api, session) = (self.fs.api.clone(), self.session.clone());
        async move {
            let _ = api.send(|http| http.delete(&session)).await;
        }
        .boxed()
    }
}

impl WriteSink for UploadSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        async move {
            self.buffer.extend(chunk);
            while self.buffer.len() > CHUNK {
                let part: Vec<u8> = self.buffer.drain(..CHUNK).collect();
                self.put(part, None).await?;
            }
            Ok(())
        }
        .boxed()
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        async move {
            let last = std::mem::take(&mut self.buffer);
            let total = self.sent + last.len() as u64;
            let result = match self.put(last, Some(total)).await {
                Ok(ChunkOutcome::Complete) => Ok(()),
                Ok(ChunkOutcome::MoreWanted) => Err(VfsError::Io { message: "Drive didn't finish the upload".into() }),
                Err(error) => Err(error),
            };
            self.fs.forget_listings();
            self.finished = result.is_ok();
            // A failed commit is a failed job, not a pause: nothing resumes it.
            self.kept = false;
            result
        }
        .boxed()
    }

    fn abort(mut self: Box<Self>) -> BoxFuture<'static, ()> {
        self.finished = true;
        self.cancel_session()
    }

    fn checkpoint(&mut self) -> Option<ResumeState> {
        self.kept = true;
        Some(ResumeState(self.session.clone()))
    }
}

impl Drop for UploadSink {
    fn drop(&mut self) {
        if !self.finished && !self.kept {
            self.fs.runtime.spawn(self.cancel_session());
        }
    }
}

impl FolderMaker for DriveFs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        self.require_absent(path).await?;
        let (parent, name) = self.destination(path).await?;
        let url = self.files_url();
        let body = json!({"name": name, "mimeType": FOLDER_MIME, "parents": [parent]});
        json(self.api.send(|http| http.post(&url).json(&body)).await?).await?;
        self.forget_listings();
        self.stat(path).await
    }
}

impl Mover for DriveFs {
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        let source = self.resolve(from).await?;
        let child = source.child.ok_or(VfsError::InvalidPath(PathError::Root))?;
        match self.require_absent(to).await {
            Err(VfsError::AlreadyExists) if from != to => return Err(VfsError::AlreadyExists),
            Err(VfsError::AlreadyExists) | Ok(()) => {}
            Err(error) => return Err(error),
        }
        let (new_parent, _) = self.destination(to).await?;
        let name = to.name().ok_or(VfsError::InvalidPath(PathError::Root))?;
        let mut query = Vec::new();
        let old_parent = source.parent_id.unwrap_or_else(|| ROOT_ID.to_owned());
        if old_parent != new_parent {
            query.push(("addParents", new_parent));
            query.push(("removeParents", old_parent));
        }
        self.patch(&child.id, query, json!({"name": child.drive_name_for(name)})).await.map(drop)
    }
}

impl Copier for DriveFs {
    /// Server-side copy of one file.
    async fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<ByteSize, VfsError> {
        let child = self.resolve(from).await?.child.ok_or(VfsError::IsADirectory)?;
        if child.content == Content::Folder {
            return Err(VfsError::IsADirectory);
        }
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        self.require_absent(to).await?;
        let (parent, _) = self.destination(to).await?;
        let name = to.name().ok_or(VfsError::InvalidPath(PathError::Root))?;
        let url = format!("{}/copy", self.file_url(&child.id));
        let body = json!({"name": child.drive_name_for(name), "parents": [parent]});
        json(self.api.send(|http| http.post(&url).json(&body)).await?).await?;
        self.forget_listings();
        let size = ByteSize(child.size.unwrap_or_default());
        progress(size);
        Ok(size)
    }
}

impl Remover for DriveFs {
    /// Deletes for good, skipping Drive's Trash (folders with everything in them).
    async fn remove(&self, path: &StoragePath, cancel: CancelToken) -> Result<(), VfsError> {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let child = self.resolve(path).await?.child.ok_or(VfsError::InvalidPath(PathError::Root))?;
        let url = self.file_url(&child.id);
        let response = self.api.send(|http| http.delete(&url)).await?;
        self.forget_listings();
        checked(response).await.map(drop)
    }
}

impl Trasher for DriveFs {
    /// Drive's own Trash; restoring puts the item back where it was.
    async fn trash(&self, path: &StoragePath) -> Result<TrashTicket, VfsError> {
        let child = self.resolve(path).await?.child.ok_or(VfsError::InvalidPath(PathError::Root))?;
        self.patch(&child.id, Vec::new(), json!({"trashed": true})).await?;
        let trashed = self.root.join(PathSegment::new(TRASH_SEGMENT)?).join(PathSegment::new(&child.id)?);
        Ok(TrashTicket { original: path.clone(), trashed })
    }

    async fn restore(&self, ticket: &TrashTicket) -> Result<(), VfsError> {
        let id = ticket.trashed.name().map(|n| n.display_name().into_owned()).ok_or(VfsError::NotInTrash)?;
        let url = self.file_url(&id);
        let item = match json(self.api.send(|http| http.get(&url).query(&[("fields", "id,trashed")])).await?).await {
            Ok(item) => item,
            Err(VfsError::NotFound) => return Err(VfsError::NotInTrash),
            Err(error) => return Err(error),
        };
        if item["trashed"].as_bool() != Some(true) {
            return Err(VfsError::NotInTrash);
        }
        self.require_absent(&ticket.original).await?;
        self.patch(&id, Vec::new(), json!({"trashed": false})).await.map(drop)
    }
}
