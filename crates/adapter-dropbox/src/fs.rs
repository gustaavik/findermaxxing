//! The Dropbox roles for one account. Dropbox addresses items by path
//! (case-insensitively), reports problems as 409 with an `error_summary`,
//! and continues upload sessions from the offset it reports.

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
use reqwest::{Response, StatusCode};
use serde_json::{Value, json};

const API_ARG: &str = "Dropbox-API-Arg";
const OCTET_STREAM: &str = "application/octet-stream";
const LIST_LIMIT: u64 = 2000;
/// Upload session chunks (a multiple of the 4 MiB Dropbox prefers).
const CHUNK: usize = 8 * 1024 * 1024;
const HIDDEN_PREFIX: u8 = b'.';
const RANGE: &str = "Range";

/// One account as a mount. Clones share the client.
#[derive(Clone)]
pub struct DropboxFs {
    api: ApiClient,
    rpc: String,
    content: String,
    root: StoragePath,
}

impl std::fmt::Debug for DropboxFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DropboxFs").field("root", &self.root).finish_non_exhaustive()
    }
}

/// JSON for an HTTP header: Dropbox wants every non-ASCII character escaped.
fn header_json(value: &Value) -> String {
    let mut out = String::new();
    for c in value.to_string().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0_u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

fn status_error(status: StatusCode, body: &str) -> VfsError {
    if status == StatusCode::CONFLICT {
        let summary = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v["error_summary"].as_str().map(str::to_owned))
            .unwrap_or_default();
        return summary_error(&summary);
    }
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => VfsError::PermissionDenied,
        StatusCode::NOT_FOUND => VfsError::NotFound,
        status if status.is_server_error() => VfsError::Transient { retry_after: None },
        status => VfsError::Io { message: format!("Dropbox answered {status}") },
    }
}

fn summary_error(summary: &str) -> VfsError {
    let has = |part: &str| summary.contains(part);
    // A closed upload session can't be continued: as good as gone.
    if has("not_found") || summary.starts_with("closed") {
        VfsError::NotFound
    } else if has("not_folder") {
        VfsError::NotADirectory
    } else if has("not_file") {
        VfsError::IsADirectory
    } else if has("conflict") {
        VfsError::AlreadyExists
    } else if has("too_many_write_operations") || has("too_many_requests") {
        VfsError::Transient { retry_after: None }
    } else if has("no_write_permission") || has("restricted_content") {
        VfsError::PermissionDenied
    } else {
        VfsError::Io { message: format!("Dropbox: {summary}") }
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
    if text.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&text).map_err(|e| VfsError::Io { message: e.to_string() })
    }
}

impl DropboxFs {
    pub(crate) fn new(api: ApiClient, rpc: String, content: String, root: StoragePath) -> Self {
        Self { api, rpc, content, root }
    }

    #[must_use]
    pub fn root(&self) -> &StoragePath {
        &self.root
    }

    /// `/a/b`, or `""` for the root. Dropbox names are Unicode.
    fn path(location: &StoragePath) -> Result<String, VfsError> {
        let mut path = String::new();
        for segment in location.segments() {
            let name = std::str::from_utf8(segment.as_bytes())
                .map_err(|_| VfsError::InvalidPath(PathError::InvalidSegment(segment.display_name().into_owned())))?;
            path.push('/');
            path.push_str(name);
        }
        Ok(path)
    }

    async fn rpc(&self, endpoint: &str, body: &Value) -> Result<Value, VfsError> {
        let url = format!("{}/2/{endpoint}", self.rpc);
        json(self.api.send(|http| http.post(&url).json(body)).await?).await
    }

    fn entry(&self, parent: &StoragePath, metadata: &Value) -> Option<FileEntry> {
        let name = PathSegment::new(metadata["name"].as_str()?).ok()?;
        let kind = match metadata[".tag"].as_str()? {
            "folder" => EntryKind::Directory,
            "file" => EntryKind::File,
            _ => return None,
        };
        let hidden = name.as_bytes().first() == Some(&HIDDEN_PREFIX);
        let metadata = EntryMetadata {
            size: (kind == EntryKind::File).then(|| ByteSize(metadata["size"].as_u64().unwrap_or_default())),
            modified: metadata["server_modified"].as_str().and_then(parse_rfc3339),
            visibility: if hidden { Visibility::Hidden } else { Visibility::Visible },
            permissions: None,
        };
        Some(FileEntry::new(parent, name, kind, metadata))
    }

    async fn metadata(&self, location: &StoragePath) -> Result<Value, VfsError> {
        self.rpc("files/get_metadata", &json!({"path": Self::path(location)?})).await
    }

    async fn require_absent(&self, location: &StoragePath) -> Result<(), VfsError> {
        match self.metadata(location).await {
            Ok(_) => Err(VfsError::AlreadyExists),
            Err(VfsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Dropbox creates missing parents; the contract wants `NotFound`.
    async fn require_folder(&self, location: &StoragePath) -> Result<(), VfsError> {
        if location.parent().is_none() {
            return Ok(());
        }
        match self.metadata(location).await?[".tag"].as_str() {
            Some("folder") => Ok(()),
            _ => Err(VfsError::NotADirectory),
        }
    }

    async fn require_parent(&self, location: &StoragePath) -> Result<(), VfsError> {
        self.require_folder(&location.parent().ok_or(VfsError::InvalidPath(PathError::Root))?).await
    }

    async fn listing(&self, dir: &StoragePath) -> Result<Vec<FileEntry>, VfsError> {
        let mut page = self.rpc("files/list_folder", &json!({"path": Self::path(dir)?, "limit": LIST_LIMIT})).await?;
        let mut entries = Vec::new();
        loop {
            entries.extend(page["entries"].as_array().into_iter().flatten().filter_map(|m| self.entry(dir, m)));
            if page["has_more"].as_bool() != Some(true) {
                return Ok(entries);
            }
            let cursor = page["cursor"].clone();
            page = self.rpc("files/list_folder/continue", &json!({"cursor": cursor})).await?;
        }
    }
}

impl Lister for DropboxFs {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let (fs, dir) = (self.clone(), dir.clone());
        stream::once(async move {
            let batches: Vec<Result<EntryBatch, VfsError>> = match fs.listing(&dir).await {
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
        let metadata = self.metadata(path).await?;
        self.entry(&parent, &metadata).ok_or(VfsError::NotFound)
    }
}

impl Reader for DropboxFs {
    async fn read(&self, path: &StoragePath, cancel: CancelToken) -> Result<ByteStream, VfsError> {
        self.read_from(path, ByteSize(0), cancel).await
    }

    async fn read_from(
        &self,
        path: &StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> Result<ByteStream, VfsError> {
        let url = format!("{}/2/files/download", self.content);
        let arg = header_json(&json!({"path": Self::path(path)?}));
        let range = (offset.0 > 0).then(|| format!("bytes={}-", offset.0));
        let response = self
            .api
            .send(|http| {
                let request = http.post(&url).header(API_ARG, &arg);
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

impl Writer for DropboxFs {
    /// Small files are one `upload`; larger ones an upload session. Both
    /// commit with `mode: add`, so nothing is ever overwritten.
    async fn create(&self, path: &StoragePath, _size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        self.require_absent(path).await?;
        self.require_parent(path).await?;
        Ok(Box::new(UploadSink {
            fs: self.clone(),
            path: Self::path(path)?,
            buffer: Vec::new(),
            session: None,
            offset: 0,
        }))
    }

    /// The state is the session id; Dropbox reports how much it has.
    async fn resume(&self, path: &StoragePath, state: &ResumeState) -> Result<ResumedSink, VfsError> {
        self.require_absent(path).await?;
        let url = format!("{}/2/files/upload_session/append_v2", self.content);
        let arg = header_json(&json!({"cursor": {"session_id": state.0, "offset": 0}, "close": false}));
        let response = self
            .api
            .send(|http| {
                http.post(&url)
                    .header(API_ARG, &arg)
                    .header(reqwest::header::CONTENT_TYPE, OCTET_STREAM)
                    .body(Vec::new())
            })
            .await?;
        let offset = if response.status().is_success() {
            0
        } else {
            let status = response.status();
            let body: Value = serde_json::from_str(&response.text().await.unwrap_or_default()).unwrap_or(Value::Null);
            match body["error"]["correct_offset"].as_u64() {
                Some(offset) => offset,
                None => return Err(status_error(status, &body.to_string())),
            }
        };
        let sink = UploadSink {
            fs: self.clone(),
            path: Self::path(path)?,
            buffer: Vec::new(),
            session: Some(state.0.clone()),
            offset,
        };
        Ok(ResumedSink { sink: Box::new(sink), offset: ByteSize(offset) })
    }

    /// Sessions can't be deleted, but they can be closed: nothing can be
    /// added any more, and Dropbox drops them after a week, unbilled.
    async fn discard(&self, path: &StoragePath, state: &ResumeState) -> Result<(), VfsError> {
        let offset = match self.resume(path, state).await {
            Ok(resumed) => resumed.offset.0,
            Err(VfsError::NotFound | VfsError::AlreadyExists) => return Ok(()),
            Err(error) => return Err(error),
        };
        let url = format!("{}/2/files/upload_session/append_v2", self.content);
        let arg = header_json(&json!({"cursor": {"session_id": state.0, "offset": offset}, "close": true}));
        let response = self
            .api
            .send(|http| {
                http.post(&url)
                    .header(API_ARG, &arg)
                    .header(reqwest::header::CONTENT_TYPE, OCTET_STREAM)
                    .body(Vec::new())
            })
            .await?;
        match checked(response).await {
            Ok(_) | Err(VfsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

struct UploadSink {
    fs: DropboxFs,
    path: String,
    buffer: Vec<u8>,
    session: Option<String>,
    /// Bytes Dropbox has received in the session.
    offset: u64,
}

impl UploadSink {
    async fn content_call(&self, endpoint: &str, arg: &Value, body: Vec<u8>) -> Result<Value, VfsError> {
        let url = format!("{}/2/{endpoint}", self.fs.content);
        let arg = header_json(arg);
        let response = self
            .fs
            .api
            .send(|http| {
                http.post(&url)
                    .header(API_ARG, &arg)
                    .header(reqwest::header::CONTENT_TYPE, OCTET_STREAM)
                    .body(body.clone())
            })
            .await?;
        json(response).await
    }

    async fn send_chunk(&mut self, chunk: Vec<u8>) -> Result<(), VfsError> {
        let length = chunk.len() as u64;
        match &self.session {
            None => {
                let started = self.content_call("files/upload_session/start", &json!({"close": false}), chunk).await?;
                self.session = Some(
                    started["session_id"]
                        .as_str()
                        .ok_or_else(|| VfsError::Io { message: "no session id".into() })?
                        .to_owned(),
                );
            }
            Some(session) => {
                let cursor = json!({"session_id": session, "offset": self.offset});
                self.content_call("files/upload_session/append_v2", &json!({"cursor": cursor, "close": false}), chunk)
                    .await?;
            }
        }
        self.offset += length;
        Ok(())
    }

    fn commit_arg(&self) -> Value {
        json!({"path": self.path, "mode": "add", "autorename": false, "mute": true})
    }
}

impl WriteSink for UploadSink {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>> {
        async move {
            self.buffer.extend(chunk);
            while self.buffer.len() > CHUNK {
                let part: Vec<u8> = self.buffer.drain(..CHUNK).collect();
                self.send_chunk(part).await?;
            }
            Ok(())
        }
        .boxed()
    }

    fn commit(mut self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>> {
        async move {
            let last = std::mem::take(&mut self.buffer);
            match &self.session {
                None => self.content_call("files/upload", &self.commit_arg(), last).await.map(drop),
                Some(session) => {
                    let arg =
                        json!({"cursor": {"session_id": session, "offset": self.offset}, "commit": self.commit_arg()});
                    self.content_call("files/upload_session/finish", &arg, last).await.map(drop)
                }
            }
        }
        .boxed()
    }

    /// Nothing is visible until commit; an abandoned session just expires.
    fn abort(self: Box<Self>) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    fn checkpoint(&mut self) -> Option<ResumeState> {
        self.session.clone().map(ResumeState)
    }
}

impl FolderMaker for DropboxFs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        self.require_absent(path).await?;
        self.require_parent(path).await?;
        self.rpc("files/create_folder_v2", &json!({"path": Self::path(path)?, "autorename": false})).await?;
        self.stat(path).await
    }
}

impl Mover for DropboxFs {
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        self.metadata(from).await?;
        // Paths are case-insensitive: a case-only rename names the same item.
        let same_item = Self::path(from)?.to_lowercase() == Self::path(to)?.to_lowercase();
        if !same_item {
            self.require_absent(to).await?;
        }
        self.require_parent(to).await?;
        let body = json!({"from_path": Self::path(from)?, "to_path": Self::path(to)?, "autorename": false});
        self.rpc("files/move_v2", &body).await.map(drop)
    }
}

impl Copier for DropboxFs {
    /// Server-side copy.
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
        self.require_absent(to).await?;
        self.require_parent(to).await?;
        let body = json!({"from_path": Self::path(from)?, "to_path": Self::path(to)?, "autorename": false});
        self.rpc("files/copy_v2", &body).await?;
        let size = source.metadata().size.unwrap_or_default();
        progress(size);
        Ok(size)
    }
}

impl Remover for DropboxFs {
    /// Deleted items stay recoverable on dropbox.com for the plan's history period.
    async fn remove(&self, path: &StoragePath, cancel: CancelToken) -> Result<(), VfsError> {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        self.rpc("files/delete_v2", &json!({"path": Self::path(path)?})).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_arguments_escape_non_ascii() {
        assert_eq!(header_json(&json!({"path": "/caf\u{e9} \u{1f389}"})), r#"{"path":"/caf\u00e9 \ud83c\udf89"}"#);
    }

    #[test]
    fn error_summaries_map_to_the_contract() {
        assert_eq!(summary_error("path/not_found/.."), VfsError::NotFound);
        assert_eq!(summary_error("path/conflict/file/.."), VfsError::AlreadyExists);
        assert_eq!(summary_error("path/not_file/"), VfsError::IsADirectory);
        assert_eq!(summary_error("too_many_write_operations/"), VfsError::Transient { retry_after: None });
    }
}
