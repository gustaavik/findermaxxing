//! Google Drive API v3: files by id, parents, duplicate names allowed,
//! Google Docs that must be exported, resumable uploads in 256 KiB chunks.

use hyper::StatusCode;
use serde_json::{Value, json};

use crate::http::{Reply, Request};
use crate::tree::{FOLDER_MIME, Item, ROOT, Tree, rfc3339};
use crate::{State, Upload};

pub const GOOGLE_APPS_PREFIX: &str = "application/vnd.google-apps.";
const CHUNK_MULTIPLE: usize = 256 * 1024;
const DEFAULT_PAGE: usize = 100;
const MAX_PAGE: usize = 1000;
/// Drive's "Client Closed Request" for a cancelled upload.
const CANCELLED_UPLOAD: u16 = 499;

fn error(status: StatusCode, reason: &str, message: &str) -> Reply {
    Reply::json(
        status,
        &json!({"error": {"code": status.as_u16(), "message": message, "errors": [{"reason": reason}]}}),
    )
}

fn not_found() -> Reply {
    error(StatusCode::NOT_FOUND, "notFound", "File not found")
}

fn resource(item: &Item) -> Value {
    let mut value = json!({
        "id": item.id,
        "name": item.name,
        "mimeType": item.mime,
        "modifiedTime": rfc3339(item.modified_ms),
        "createdTime": rfc3339(item.modified_ms),
        "parents": [item.parent],
        "trashed": item.trashed,
    });
    if !item.folder && !item.mime.starts_with(GOOGLE_APPS_PREFIX) {
        // Drive sends sizes as strings.
        value["size"] = item.content.len().to_string().into();
    }
    value
}

/// Accepts the one query form the adapter sends: `'<id>' in parents and trashed = false`.
fn parent_of_query(q: &str) -> Option<String> {
    let start = q.find('\'')? + 1;
    let end = start + q[start..].find('\'')?;
    q[end..].contains("in parents").then(|| q[start..end].to_owned())
}

pub fn handle(state: &mut State, rest: &str, request: &Request) -> Reply {
    let base = state.base.clone();
    let parts: Vec<&str> = rest.split('/').collect();
    if request.method == "POST" && parts == ["upload", "drive", "v3", "files"] {
        return start_upload(state, &base, request);
    }
    let tree = &mut state.google;
    match (request.method.as_str(), parts.as_slice()) {
        ("GET", ["drive", "v3", "about"]) => {
            Reply::ok(&json!({"user": {"displayName": "Mock User", "emailAddress": "mock@gmail.com"}}))
        }
        ("GET", ["drive", "v3", "files"]) => list(tree, request),
        ("POST", ["drive", "v3", "files"]) => create(tree, &request.json()),
        ("GET", ["drive", "v3", "files", id]) if request.query.get("alt").is_some_and(|a| a == "media") => {
            download(tree, id, request)
        }
        // Trashed items are still there by id, with `trashed: true`.
        ("GET", ["drive", "v3", "files", id]) => {
            tree.get_any(id).map_or_else(not_found, |item| Reply::ok(&resource(item)))
        }
        ("PATCH", ["drive", "v3", "files", id]) => update(tree, id, request),
        ("DELETE", ["drive", "v3", "files", id]) => {
            if tree.get_any(id).is_none() || *id == ROOT {
                return not_found();
            }
            tree.remove_tree(id);
            Reply::empty(StatusCode::NO_CONTENT)
        }
        ("POST", ["drive", "v3", "files", id, "copy"]) => copy(tree, id, &request.json()),
        ("GET", ["drive", "v3", "files", id, "export"]) => {
            let Some(item) = tree.get(id) else { return not_found() };
            let mime = request.query.get("mimeType").cloned().unwrap_or_default();
            if !item.mime.starts_with(GOOGLE_APPS_PREFIX) || item.folder {
                return error(StatusCode::FORBIDDEN, "fileNotExportable", "Export only supports Docs Editors files.");
            }
            Reply::bytes(StatusCode::OK, format!("EXPORT:{}:{mime}", item.name).into_bytes())
        }
        _ => Reply::empty(StatusCode::NOT_FOUND),
    }
}

fn list(tree: &Tree, request: &Request) -> Reply {
    let Some(parent) = request.query.get("q").and_then(|q| parent_of_query(q)) else {
        return error(StatusCode::BAD_REQUEST, "invalid", "unsupported query");
    };
    if tree.get(&parent).is_none() {
        return not_found();
    }
    let size = request.query.get("pageSize").and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_PAGE).min(MAX_PAGE);
    let start: usize = request.query.get("pageToken").and_then(|t| t.parse().ok()).unwrap_or(0);
    let children = tree.children(&parent);
    let page: Vec<Value> = children.iter().skip(start).take(size).map(resource).collect();
    let mut body = json!({"files": page});
    if start + size < children.len() {
        body["nextPageToken"] = (start + size).to_string().into();
    }
    Reply::ok(&body)
}

fn parents_of(metadata: &Value) -> String {
    metadata["parents"].get(0).and_then(Value::as_str).unwrap_or(ROOT).to_owned()
}

fn create(tree: &mut Tree, metadata: &Value) -> Reply {
    let parent = parents_of(metadata);
    if tree.get(&parent).is_none_or(|p| !p.folder) {
        return not_found();
    }
    let name = metadata["name"].as_str().unwrap_or("Untitled").to_owned();
    let mime = metadata["mimeType"].as_str().unwrap_or("application/octet-stream").to_owned();
    let id = tree.add(&parent, &name, mime == FOLDER_MIME, Vec::new(), &mime);
    tree.get(&id).map_or_else(not_found, |item| Reply::ok(&resource(item)))
}

fn download(tree: &Tree, id: &str, request: &Request) -> Reply {
    let Some(item) = tree.get(id) else { return not_found() };
    if item.mime.starts_with(GOOGLE_APPS_PREFIX) {
        let message = if item.folder {
            "folders have no content"
        } else {
            "Only files with binary content can be downloaded. Use Export with Docs Editors files."
        };
        return error(StatusCode::FORBIDDEN, "fileNotDownloadable", message);
    }
    Reply::content(&item.content, request.range_start())
}

fn update(tree: &mut Tree, id: &str, request: &Request) -> Reply {
    if tree.get_any(id).is_none() || id == ROOT {
        return not_found();
    }
    let body = request.json();
    if let Some(new_parent) = request.query.get("addParents") {
        if tree.get(new_parent).is_none_or(|p| !p.folder) {
            return not_found();
        }
        if tree.is_within(new_parent, id) {
            return error(StatusCode::BAD_REQUEST, "invalidParent", "can't move a folder into itself");
        }
    }
    let Some(item) = tree.get_mut(id) else { return not_found() };
    if let Some(new_parent) = request.query.get("addParents") {
        item.parent.clone_from(new_parent);
    }
    if let Some(name) = body["name"].as_str() {
        item.name = name.to_owned();
    }
    if let Some(trashed) = body["trashed"].as_bool() {
        item.trashed = trashed;
    }
    item.modified_ms = crate::tree::now_ms();
    let item = item.clone();
    Reply::ok(&resource(&item))
}

fn copy(tree: &mut Tree, id: &str, metadata: &Value) -> Reply {
    let Some(item) = tree.get(id).cloned() else { return not_found() };
    if item.folder {
        return error(StatusCode::FORBIDDEN, "cannotCopyFolder", "folders can't be copied");
    }
    let parent = parents_of(metadata);
    if tree.get(&parent).is_none_or(|p| !p.folder) {
        return not_found();
    }
    let name = metadata["name"].as_str().unwrap_or(&item.name).to_owned();
    let copy = tree.add(&parent, &name, false, item.content, &item.mime);
    tree.get(&copy).map_or_else(not_found, |item| Reply::ok(&resource(item)))
}

fn start_upload(state: &mut State, base: &str, request: &Request) -> Reply {
    let metadata = request.json();
    let parent = parents_of(&metadata);
    if state.google.get(&parent).is_none_or(|p| !p.folder) {
        return not_found();
    }
    state.next_upload += 1;
    let id = format!("gup-{}", state.next_upload);
    let total = request.header("x-upload-content-length").and_then(|t| t.parse().ok());
    let name = metadata["name"].as_str().unwrap_or("Untitled").to_owned();
    state.uploads.insert(id.clone(), Upload { parent, name, received: Vec::new(), total, closed: false });
    let location = format!("{base}/gdrive/upload/drive/v3/files?uploadType=resumable&upload_id={id}");
    Reply::empty(StatusCode::OK).with_header("location", &location)
}

/// `bytes a-b/total`, `bytes a-b/*` or `bytes */total` → (start, total).
fn content_range(value: &str) -> Option<(Option<u64>, Option<u64>)> {
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let total = if total == "*" { None } else { Some(total.parse().ok()?) };
    let start = if range == "*" { None } else { Some(range.split('-').next()?.parse().ok()?) };
    Some((start, total))
}

pub fn handle_upload(state: &mut State, request: &Request) -> Reply {
    let Some(id) = request.query.get("upload_id").cloned() else { return not_found() };
    if request.method == "DELETE" {
        state.uploads.remove(&id);
        return Reply::empty(StatusCode::from_u16(CANCELLED_UPLOAD).unwrap_or(StatusCode::BAD_REQUEST));
    }
    let Some(upload) = state.uploads.get_mut(&id) else { return not_found() };
    let (start, total) = match request.header("content-range").map(content_range) {
        Some(Some(range)) => range,
        Some(None) => {
            return error(StatusCode::BAD_REQUEST, "badContentRange", "Failed to parse Content-Range header.");
        }
        // A single request with the whole file.
        None => (Some(0), Some(request.body.len() as u64)),
    };
    if let Some(total) = total {
        upload.total = Some(total);
    }
    if let Some(start) = start {
        if start != upload.received.len() as u64 {
            return error(StatusCode::BAD_REQUEST, "badContentRange", "chunk doesn't continue the upload");
        }
        let last = upload.total.is_some_and(|t| start + request.body.len() as u64 == t);
        if !last && !request.body.len().is_multiple_of(CHUNK_MULTIPLE) {
            return error(StatusCode::BAD_REQUEST, "badChunk", "chunks must be multiples of 256 KiB");
        }
        upload.received.extend_from_slice(&request.body);
    }
    if upload.total == Some(upload.received.len() as u64) {
        let upload = state.uploads.remove(&id).unwrap_or_else(|| unreachable!("present above"));
        if state.google.get(&upload.parent).is_none() {
            return not_found();
        }
        let file = state.google.add(&upload.parent, &upload.name, false, upload.received, "application/octet-stream");
        return state.google.get(&file).map_or_else(not_found, |item| Reply::ok(&resource(item)));
    }
    let reply = Reply::empty(StatusCode::PERMANENT_REDIRECT);
    if upload.received.is_empty() {
        reply
    } else {
        reply.with_header("range", &format!("bytes=0-{}", upload.received.len() - 1))
    }
}

/// Seeds a Google Doc (no content; must be exported).
pub fn add_google_doc(tree: &mut Tree, parent: &str, name: &str, kind: &str) -> String {
    tree.add(parent, name, false, Vec::new(), &format!("{GOOGLE_APPS_PREFIX}{kind}"))
}
