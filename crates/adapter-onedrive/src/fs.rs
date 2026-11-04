//! The OneDrive roles for one account (Microsoft Graph). Items are
//! addressed by path (`root:/a/b:`) for lookups and by id for changes.
//! Upload sessions use pre-authorised URLs that must not get the bearer
//! token, take chunks in multiples of 320 KiB, and need the total size in
//! every chunk; the resume state therefore carries the size with the URL.

use std::time::Duration;

use application::CancelToken;
use application::ports::{
    ByteStream, Copier, FolderMaker, Lister, Mover, ProgressCallback, Reader, Remover, ResumeState, ResumedSink,
    WriteSink, Writer, skip_bytes,
};
use domain::{
    ByteSize, EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, PathError, PathSegment, StoragePath,
    VfsError, Visibility,
};
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use futures::{FutureExt, StreamExt};
use infra_oauth::{ApiClient, network_error, parse_rfc3339};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Response, StatusCode};
use serde_json::{Value, json};
use tokio::runtime::Handle;

const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
const CONFLICT_BEHAVIOR: &str = "@microsoft.graph.conflictBehavior";
/// 32 × 320 KiB: Graph wants chunks in multiples of 320 KiB.
const CHUNK: usize = 32 * 320 * 1024;
/// Smaller files go up in one request.
const SIMPLE_UPLOAD_LIMIT: usize = 4 * 1024 * 1024;
const PAGE_SIZE: &str = "999";
const HIDDEN_PREFIX: u8 = b'.';
const RANGE: &str = "Range";
const CONTENT_RANGE: &str = "Content-Range";
const COPY_POLL: Duration = Duration::from_millis(500);
const COPY_POLLS: u32 = 240;

/// One account as a mount. Clones share the client.
#[derive(Clone)]
pub struct OneDriveFs {
    api: ApiClient,
    base: String,
    root: StoragePath,
    runtime: Handle,
}

impl std::fmt::Debug for OneDriveFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OneDriveFs").field("root", &self.root).finish_non_exhaustive()
    }
}

fn status_error(status: StatusCode, body: &str) -> VfsError {
    let code = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_owned))
        .unwrap_or_default();
    match (status, code.as_str()) {
        (_, "nameAlreadyExists") | (StatusCode::CONFLICT, _) => VfsError::AlreadyExists,
        (StatusCode::NOT_FOUND, _) | (_, "itemNotFound") => VfsError::NotFound,
        (StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN, _) | (_, "accessDenied") => VfsError::PermissionDenied,
        (StatusCode::INSUFFICIENT_STORAGE, _) | (_, "quotaLimitReached") => {
            VfsError::Io { message: "the OneDrive is full".into() }
        }
        (status, _) if status.is_server_error() => VfsError::Transient { retry_after: None },
        (status, code) => VfsError::Io { message: format!("OneDrive answered {status} {code}") },
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

impl OneDriveFs {
    pub(crate) fn new(api: ApiClient, base: String, root: StoragePath, runtime: Handle) -> Self {
        Self { api, base, root, runtime }
    }

    #[must_use]
    pub fn root(&self) -> &StoragePath {
        &self.root
    }

    fn encoded(location: &StoragePath) -> Result<String, VfsError> {
        let mut parts = Vec::new();
        for segment in location.segments() {
            let name = std::str::from_utf8(segment.as_bytes())
                .map_err(|_| VfsError::InvalidPath(PathError::InvalidSegment(segment.display_name().into_owned())))?;
            parts.push(utf8_percent_encode(name, SEGMENT).to_string());
        }
        Ok(parts.join("/"))
    }

    /// `…/root` or `…/root:/a/b:`, to which `/children` or `/content` can be appended.
    fn item_url(&self, location: &StoragePath) -> Result<String, VfsError> {
        Ok(if location.parent().is_none() {
            format!("{}/me/drive/root", self.base)
        } else {
            format!("{}/me/drive/root:/{}:", self.base, Self::encoded(location)?)
        })
    }

    fn by_id(&self, id: &str) -> String {
        format!("{}/me/drive/items/{id}", self.base)
    }

    async fn item(&self, location: &StoragePath) -> Result<Value, VfsError> {
        let url = self.item_url(location)?;
        json(self.api.send(|http| http.get(&url)).await?).await
    }

    async fn require_absent(&self, location: &StoragePath) -> Result<(), VfsError> {
        match self.item(location).await {
            Ok(_) => Err(VfsError::AlreadyExists),
            Err(VfsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The id of the folder at `location`.
    async fn folder_id(&self, location: &StoragePath) -> Result<String, VfsError> {
        let item = self.item(location).await?;
        if item.get("folder").is_none() {
            return Err(VfsError::NotADirectory);
        }
        item["id"].as_str().map(str::to_owned).ok_or(VfsError::NotFound)
    }

    async fn parent_id(&self, location: &StoragePath) -> Result<String, VfsError> {
        self.folder_id(&location.parent().ok_or(VfsError::InvalidPath(PathError::Root))?).await
    }

    fn entry(parent: &StoragePath, item: &Value) -> Option<FileEntry> {
        let name = PathSegment::new(item["name"].as_str()?).ok()?;
        let kind = if item.get("folder").is_some() {
            EntryKind::Directory
        } else if item.get("file").is_some() {
            EntryKind::File
        } else {
            // OneNote notebooks and other packages: listed, not readable.
            EntryKind::Other
        };
        let hidden = name.as_bytes().first() == Some(&HIDDEN_PREFIX);
        let metadata = EntryMetadata {
            size: (kind == EntryKind::File).then(|| ByteSize(item["size"].as_u64().unwrap_or_default())),
            modified: item["lastModifiedDateTime"].as_str().and_then(parse_rfc3339),
            visibility: if hidden { Visibility::Hidden } else { Visibility::Visible },
            permissions: None,
        };
        Some(FileEntry::new(parent, name, kind, metadata))
    }

    async fn children(&self, dir: &StoragePath) -> Result<Vec<FileEntry>, VfsError> {
        self.folder_id(dir).await?;
        let mut url = format!("{}/children?$top={PAGE_SIZE}", self.item_url(dir)?);
        let mut entries = Vec::new();
        loop {
            let page = json(self.api.send(|http| http.get(&url)).await?).await?;
            entries.extend(page["value"].as_array().into_iter().flatten().filter_map(|item| Self::entry(dir, item)));
            match page["@odata.nextLink"].as_str() {
                Some(next) => url = next.to_owned(),
                None => return Ok(entries),
            }
        }
    }

    /// Follows a copy's monitor URL (pre-authorised) until it ends.
    async fn await_copy(&self, monitor: &str) -> Result<(), VfsError> {
        for _ in 0..COPY_POLLS {
            let response = self.api.http.get(monitor).send().await.map_err(|e| network_error(&e))?;
            let status: Value = json(response).await?;
            match status["status"].as_str() {
                Some("completed") => return Ok(()),
                Some("failed") => {
                    return Err(status_error(StatusCode::BAD_REQUEST, &json!({"error": status["error"]}).to_string()));
                }
                _ => tokio::time::sleep(COPY_POLL).await,
            }
        }
        Err(VfsError::Transient { retry_after: None })
    }
}

impl Lister for OneDriveFs {
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
        let parent = path.parent().ok_or(VfsError::InvalidPath(PathError::Root))?;
        Self::entry(&parent, &self.item(path).await?).ok_or(VfsError::NotFound)
    }
}

impl Reader for OneDriveFs {
    async fn read(&self, path: &StoragePath, cancel: CancelToken) -> Result<ByteStream, VfsError> {
        self.read_from(path, ByteSize(0), cancel).await
    }

    async fn read_from(
        &self,
        path: &StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> Result<ByteStream, VfsError> {
        let item = self.item(path).await?;
        if item.get("folder").is_some() {
            return Err(VfsError::IsADirectory);
        }
        let url = format!("{}/content", self.item_url(path)?);
        let range = (offset.0 > 0).then(|| format!("bytes={}-", offset.0));
        // Redirects to a pre-authorised download URL; the token isn't forwarded.
        let response = self
            .api
            .send(|http| {
                let request = http.get(&url);
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
        let body = checked(response)
            .await?
            .bytes_stream()
            .map(move |chunk| {
                if cancel.is_cancelled() {
                    return Err(VfsError::Cancelled);
                }
                chunk.map(|bytes| bytes.to_vec()).map_err(|e| network_error(&e))
            })
            .boxed();
        Ok(if ranged || offset.0 == 0 { body } else { skip_bytes(body, offset) })
    }
}

/// `"<total> <upload URL>"`.
fn session_state(total: u64, url: &str) -> ResumeState {
    ResumeState(format!("{total} {url}"))
}

fn parse_state(state: &ResumeState) -> Result<(u64, String), VfsError> {
    let (total, url) = state.0.split_once(' ').ok_or(VfsError::NotFound)?;
    Ok((total.parse().map_err(|_| VfsError::NotFound)?, url.to_owned()))
}

impl Writer for OneDriveFs {
    /// Small files are one request at commit; larger ones (of known size) an
    /// upload session with `conflictBehavior: fail`, visible once complete.
    async fn create(&self, path: &StoragePath, size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        self.require_absent(path).await?;
        let parent = self.parent_id(path).await?;
        Ok(Box::new(UploadSink {
            fs: self.clone(),
            path: path.clone(),
            parent,
            total: size_hint.map(|s| s.0),
            buffer: Vec::new(),
            sent: 0,
            session: None,
            kept: false,
            finished: false,
        }))
    }

    async fn resume(&self, path: &StoragePath, state: &ResumeState) -> Result<ResumedSink, VfsError> {
        self.require_absent(path).await?;
        let (total, url) = parse_state(state)?;
        let status = json(self.api.http.get(&url).send().await.map_err(|e| network_error(&e))?).await?;
        let next = status["nextExpectedRanges"][0]
            .as_str()
            .and_then(|range| range.split('-').next()?.parse::<u64>().ok())
            .ok_or(VfsError::NotFound)?;
        let sink = UploadSink {
            fs: self.clone(),
            path: path.clone(),
            parent: String::new(),
            total: Some(total),
            buffer: Vec::new(),
            sent: next,
            session: Some(url),
            kept: true,
            finished: false,
        };
        Ok(ResumedSink { sink: Box::new(sink), offset: ByteSize(next) })
    }

    async fn discard(&self, _path: &StoragePath, state: &ResumeState) -> Result<(), VfsError> {
        let (_, url) = parse_state(state)?;
        let response = self.api.http.delete(&url).send().await.map_err(|e| network_error(&e))?;
        match checked(response).await {
            Ok(_) | Err(VfsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

struct UploadSink {
    fs: OneDriveFs,
    path: StoragePath,
    parent: String,
    /// Graph needs the total in every chunk; unknown totals are buffered.
    total: Option<u64>,
    buffer: Vec<u8>,
    sent: u64,
    session: Option<String>,
    kept: bool,
    finished: bool,
}

impl UploadSink {
    async fn session_url(&mut self, total: u64) -> Result<String, VfsError> {
        if let Some(url) = &self.session {
            return Ok(url.clone());
        }
        let name = self.path.name().ok_or(VfsError::InvalidPath(PathError::Root))?;
        let name = utf8_percent_encode(&name.display_name(), SEGMENT).to_string();
        let url = format!("{}:/{name}:/createUploadSession", self.fs.by_id(&self.parent));
        let body = json!({"item": {CONFLICT_BEHAVIOR: "fail"}});
        let created = json(self.fs.api.send(|http| http.post(&url).json(&body)).await?).await?;
        let session =
            created["uploadUrl"].as_str().ok_or_else(|| VfsError::Io { message: "no upload URL".into() })?.to_owned();
        self.total = Some(total);
        self.session = Some(session.clone());
        Ok(session)
    }

    /// Sends one chunk to the pre-authorised URL (no bearer token).
    async fn put(&mut self, chunk: Vec<u8>, total: u64) -> Result<Response, VfsError> {
        let url = self.session_url(total).await?;
        let end = self.sent + chunk.len() as u64 - 1;
        let length = chunk.len() as u64;
        let response = self
            .fs
            .api
            .http
            .put(&url)
            .header(CONTENT_RANGE, format!("bytes {}-{end}/{total}", self.sent))
            .body(chunk)
            .send()
            .await
            .map_err(|e| network_error(&e))?;
        let response = checked(response).await?;
        self.sent += length;
        Ok(response)
    }

    async fn simple_upload(&self, content: Vec<u8>) -> Result<(), VfsError> {
        let url = format!("{}/content", self.fs.item_url(&self.path)?);
        let response =
            self.fs.api.send(|http| http.put(&url).query(&[(CONFLICT_BEHAVIOR, "fail")]).body(content.clone())).await?;
        checked(response).await.map(drop)
    }

    fn cancel_session(&self) -> BoxFuture<'static, ()> {
        let (http, session) = (self.fs.api.http.clone(), self.session.clone());
        async move {
            if let Some(url) = session {
                let _ = http.delete(&url).send().await;
            }
        }
        .boxed()
    }
}

impl WriteSink for UploadSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        async move {
            self.buffer.extend(chunk);
            let Some(total) = self.total.filter(|t| *t > SIMPLE_UPLOAD_LIMIT as u64) else { return Ok(()) };
            // The file's last chunk is kept for commit, which completes the upload.
            while self.buffer.len() >= CHUNK && self.sent + (CHUNK as u64) < total {
                let part: Vec<u8> = self.buffer.drain(..CHUNK).collect();
                self.put(part, total).await?;
            }
            Ok(())
        }
        .boxed()
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        async move {
            let last = std::mem::take(&mut self.buffer);
            let total = self.sent + last.len() as u64;
            let result = if self.session.is_none() && last.len() <= SIMPLE_UPLOAD_LIMIT {
                self.simple_upload(last).await
            } else if self.total.is_some_and(|t| t != total) {
                Err(VfsError::Io { message: "the file changed size while uploading".into() })
            } else {
                // Unknown sizes were buffered: send what's left in chunks.
                let mut rest = last;
                let mut outcome = Ok(());
                while !rest.is_empty() && outcome.is_ok() {
                    let part: Vec<u8> = rest.drain(..CHUNK.min(rest.len())).collect();
                    outcome = self.put(part, total).await.map(drop);
                }
                outcome
            };
            self.finished = result.is_ok();
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
        let state = session_state(self.total?, self.session.as_ref()?);
        self.kept = true;
        Some(state)
    }
}

impl Drop for UploadSink {
    fn drop(&mut self) {
        if !self.finished && !self.kept && self.session.is_some() {
            self.fs.runtime.spawn(self.cancel_session());
        }
    }
}

impl FolderMaker for OneDriveFs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        self.require_absent(path).await?;
        let parent = self.parent_id(path).await?;
        let name = path.name().ok_or(VfsError::InvalidPath(PathError::Root))?.display_name().into_owned();
        let url = format!("{}/children", self.by_id(&parent));
        let body = json!({"name": name, "folder": {}, CONFLICT_BEHAVIOR: "fail"});
        json(self.api.send(|http| http.post(&url).json(&body)).await?).await?;
        self.stat(path).await
    }
}

impl Mover for OneDriveFs {
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        let item = self.item(from).await?;
        let id = item["id"].as_str().ok_or(VfsError::NotFound)?.to_owned();
        // Names are case-insensitive: a case-only rename names the same item.
        let same_item = from.parent() == to.parent()
            && from.name().map(|n| n.display_name().to_lowercase())
                == to.name().map(|n| n.display_name().to_lowercase());
        if !same_item {
            self.require_absent(to).await?;
        }
        let parent = self.parent_id(to).await?;
        let name = to.name().ok_or(VfsError::InvalidPath(PathError::Root))?.display_name().into_owned();
        let url = self.by_id(&id);
        let body = json!({"name": name, "parentReference": {"id": parent}});
        json(self.api.send(|http| http.patch(&url).json(&body)).await?).await.map(drop)
    }
}

impl Copier for OneDriveFs {
    /// Server-side copy; Graph runs it asynchronously behind a monitor URL.
    async fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<ByteSize, VfsError> {
        let item = self.item(from).await?;
        if item.get("folder").is_some() {
            return Err(VfsError::IsADirectory);
        }
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        self.require_absent(to).await?;
        let parent = self.parent_id(to).await?;
        let name = to.name().ok_or(VfsError::InvalidPath(PathError::Root))?.display_name().into_owned();
        let url = format!("{}/copy", self.by_id(item["id"].as_str().ok_or(VfsError::NotFound)?));
        let body = json!({"parentReference": {"id": parent}, "name": name});
        let response = checked(self.api.send(|http| http.post(&url).json(&body)).await?).await?;
        let monitor = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| VfsError::Io { message: "no copy monitor".into() })?
            .to_owned();
        self.await_copy(&monitor).await?;
        let size = ByteSize(item["size"].as_u64().unwrap_or_default());
        progress(size);
        Ok(size)
    }
}

impl Remover for OneDriveFs {
    /// Items go to the OneDrive recycle bin (restorable on onedrive.com).
    async fn remove(&self, path: &StoragePath, cancel: CancelToken) -> Result<(), VfsError> {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        let item = self.item(path).await?;
        let url = self.by_id(item["id"].as_str().ok_or(VfsError::NotFound)?);
        checked(self.api.send(|http| http.delete(&url)).await?).await.map(drop)
    }
}
