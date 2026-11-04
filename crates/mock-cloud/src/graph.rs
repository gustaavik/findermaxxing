//! Microsoft Graph (OneDrive): items by id or by `root:/path:`, downloads
//! via a redirect, pre-authorised upload sessions in 320 KiB multiples,
//! asynchronous copies with a monitor URL.

use hyper::StatusCode;
use serde_json::{Value, json};

use crate::http::{Reply, Request};
use crate::tree::{FOLDER_MIME, Item, ROOT, Tree, rfc3339};
use crate::{State, Upload};

const CHUNK_MULTIPLE: usize = 320 * 1024;
const DEFAULT_TOP: usize = 200;
const CONFLICT_BEHAVIOR: &str = "@microsoft.graph.conflictBehavior";

fn error(status: StatusCode, code: &str) -> Reply {
    Reply::json(status, &json!({"error": {"code": code, "message": code}}))
}

fn not_found() -> Reply {
    error(StatusCode::NOT_FOUND, "itemNotFound")
}

fn resource(tree: &Tree, item: &Item) -> Value {
    let mut value = json!({
        "id": item.id,
        "name": if item.id == ROOT { "root".to_owned() } else { item.name.clone() },
        "size": if item.folder { 0 } else { item.content.len() },
        "lastModifiedDateTime": rfc3339(item.modified_ms),
        "parentReference": {"id": item.parent, "path": format!("/drive/root:{}", tree.path_of(&item.parent))},
    });
    if item.folder {
        value["folder"] = json!({"childCount": tree.children(&item.id).len()});
    } else {
        value["file"] = json!({"mimeType": item.mime});
    }
    value
}

fn decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a `me/drive/…` URL addresses, and what it asks for.
enum Target {
    Id(String, String),
    Path(String, String),
}

fn target(rest: &str) -> Option<Target> {
    let drive = rest.strip_prefix("me/drive/")?;
    if let Some(path) = drive.strip_prefix("root:") {
        let (path, action) = path.split_once(":/").unwrap_or((path.trim_end_matches(':'), ""));
        return Some(Target::Path(decode(path), action.to_owned()));
    }
    if let Some(action) = drive.strip_prefix("root") {
        return Some(Target::Id(ROOT.into(), action.trim_start_matches('/').to_owned()));
    }
    let item = drive.strip_prefix("items/")?;
    let (id, action) = match item.find([':', '/']) {
        Some(index) => (&item[..index], item[index..].trim_start_matches('/')),
        None => (item, ""),
    };
    Some(Target::Id(id.to_owned(), decode(action)))
}

pub fn handle(state: &mut State, rest: &str, request: &Request) -> Reply {
    if rest == "me" {
        return Reply::ok(
            &json!({"displayName": "Mock Graph User", "mail": "mock@outlook.test", "userPrincipalName": "mock@outlook.test"}),
        );
    }
    let base = state.base.clone();
    let Some(target) = target(rest) else { return Reply::empty(StatusCode::NOT_FOUND) };
    let (id, action) = match target {
        Target::Id(id, action) => (Some(id), action),
        Target::Path(path, action) => (
            state.onedrive.by_path(&path, true),
            match (request.method.as_str(), action.as_str()) {
                // Simple upload by path: the item doesn't exist yet.
                ("PUT", "content") => return simple_upload(&mut state.onedrive, &path, request),
                _ => action,
            },
        ),
    };
    let Some(id) = id.filter(|id| state.onedrive.get(id).is_some()) else { return not_found() };
    match (request.method.as_str(), action.as_str()) {
        ("GET", "") => {
            state.onedrive.get(&id).map_or_else(not_found, |item| Reply::ok(&resource(&state.onedrive, item)))
        }
        ("GET", "children") => children(&state.onedrive, &base, rest, &id, request),
        ("POST", "children") => create_folder(&mut state.onedrive, &id, &request.json()),
        ("GET", "content") => {
            if state.onedrive.get(&id).is_some_and(|item| item.folder) {
                return error(StatusCode::BAD_REQUEST, "notSupported");
            }
            Reply::empty(StatusCode::FOUND).with_header("location", &format!("{base}/graph/download/{id}"))
        }
        ("PATCH", "") => update(&mut state.onedrive, &id, &request.json()),
        ("DELETE", "") if id != ROOT => {
            state.onedrive.remove_tree(&id);
            Reply::empty(StatusCode::NO_CONTENT)
        }
        ("POST", "copy") => copy(state, &base, &id, &request.json()),
        ("POST", action) if action.ends_with(":/createUploadSession") => {
            let name = action
                .trim_start_matches(':')
                .trim_start_matches('/')
                .trim_end_matches(":/createUploadSession")
                .to_owned();
            create_session(state, &base, &id, &name, &request.json())
        }
        _ => Reply::empty(StatusCode::NOT_FOUND),
    }
}

fn children(tree: &Tree, base: &str, rest: &str, id: &str, request: &Request) -> Reply {
    if tree.get(id).is_some_and(|item| !item.folder) {
        return error(StatusCode::BAD_REQUEST, "invalidRequest");
    }
    let top = request.query.get("$top").and_then(|t| t.parse().ok()).unwrap_or(DEFAULT_TOP);
    let skip: usize = request.query.get("$skiptoken").and_then(|t| t.parse().ok()).unwrap_or(0);
    let all = tree.children(id);
    let value: Vec<Value> = all.iter().skip(skip).take(top).map(|item| resource(tree, item)).collect();
    let mut body = json!({"value": value});
    if skip + top < all.len() {
        body["@odata.nextLink"] = format!("{base}/graph/v1.0/{rest}?$top={top}&$skiptoken={}", skip + top).into();
    }
    Reply::ok(&body)
}

fn create_folder(tree: &mut Tree, parent: &str, body: &Value) -> Reply {
    let name = body["name"].as_str().unwrap_or_default().to_owned();
    if tree.child_named(parent, &name, true).is_some() {
        return error(StatusCode::CONFLICT, "nameAlreadyExists");
    }
    let id = tree.add(parent, &name, true, Vec::new(), FOLDER_MIME);
    tree.get(&id).map_or_else(not_found, |item| Reply::json(StatusCode::CREATED, &resource(tree, item)))
}

fn update(tree: &mut Tree, id: &str, body: &Value) -> Reply {
    let Some(item) = tree.get(id).cloned() else { return not_found() };
    let parent = body["parentReference"]["id"].as_str().map_or(item.parent.clone(), str::to_owned);
    let name = body["name"].as_str().map_or(item.name, str::to_owned);
    if tree.get(&parent).is_none_or(|p| !p.folder) {
        return not_found();
    }
    if tree.is_within(&parent, id) {
        return error(StatusCode::BAD_REQUEST, "invalidRequest");
    }
    if tree.child_named(&parent, &name, true).is_some_and(|other| other.id != id) {
        return error(StatusCode::CONFLICT, "nameAlreadyExists");
    }
    if let Some(item) = tree.get_mut(id) {
        item.parent = parent;
        item.name = name;
    }
    tree.get(id).map_or_else(not_found, |item| Reply::ok(&resource(tree, item)))
}

fn copy(state: &mut State, base: &str, id: &str, body: &Value) -> Reply {
    let tree = &mut state.onedrive;
    let Some(item) = tree.get(id).cloned() else { return not_found() };
    let parent = body["parentReference"]["id"].as_str().map_or(item.parent.clone(), str::to_owned);
    let name = body["name"].as_str().map_or(item.name, str::to_owned);
    if tree.get(&parent).is_none() {
        return not_found();
    }
    state.next_upload += 1;
    let monitor = format!("mon-{}", state.next_upload);
    let outcome = if tree.child_named(&parent, &name, true).is_some() {
        json!({"status": "failed", "error": {"code": "nameAlreadyExists"}})
    } else {
        let copy = tree.copy_tree(id, &parent, &name).unwrap_or_default();
        json!({"status": "completed", "resourceId": copy})
    };
    state.uploads.insert(
        monitor.clone(),
        Upload { parent: outcome.to_string(), name: String::new(), received: Vec::new(), total: None, closed: false },
    );
    Reply::empty(StatusCode::ACCEPTED).with_header("location", &format!("{base}/graph/monitor/{monitor}"))
}

fn simple_upload(tree: &mut Tree, path: &str, request: &Request) -> Reply {
    let fail = request.query.get(CONFLICT_BEHAVIOR).is_some_and(|b| b == "fail");
    if let Some(existing) = tree.by_path(path, true) {
        if fail {
            return error(StatusCode::CONFLICT, "nameAlreadyExists");
        }
        tree.remove_tree(&existing);
    }
    let (parent_path, name) = path.rsplit_once('/').unwrap_or(("", path));
    let parent = tree.ensure_folders(parent_path, true);
    let id = tree.add(&parent, name, false, request.body.to_vec(), "application/octet-stream");
    tree.get(&id).map_or_else(not_found, |item| Reply::json(StatusCode::CREATED, &resource(tree, item)))
}

fn create_session(state: &mut State, base: &str, parent: &str, name: &str, body: &Value) -> Reply {
    let name = decode(name);
    let fail = body["item"][CONFLICT_BEHAVIOR].as_str() == Some("fail");
    if fail && state.onedrive.child_named(parent, &name, true).is_some() {
        return error(StatusCode::CONFLICT, "nameAlreadyExists");
    }
    state.next_upload += 1;
    let id = format!("gsess-{}", state.next_upload);
    state.uploads.insert(
        id.clone(),
        Upload { parent: parent.to_owned(), name, received: Vec::new(), total: None, closed: false },
    );
    Reply::ok(&json!({"uploadUrl": format!("{base}/graph/upload/{id}"), "expirationDateTime": "2030-01-01T00:00:00Z"}))
}

fn next_expected(upload: &Upload) -> Value {
    json!({"nextExpectedRanges": [format!("{}-", upload.received.len())]})
}

/// Upload sessions, downloads and copy monitors: URLs that carry their own authorisation.
pub fn handle_presigned(state: &mut State, path: &str, id: &str, request: &Request) -> Reply {
    if path.starts_with("/graph/download/") {
        return state
            .onedrive
            .get(id)
            .map_or_else(not_found, |item| Reply::content(&item.content, request.range_start()));
    }
    let Some(upload) = state.uploads.get_mut(id) else { return not_found() };
    match request.method.as_str() {
        "GET" => Reply::ok(&next_expected(upload)),
        "DELETE" => {
            state.uploads.remove(id);
            Reply::empty(StatusCode::NO_CONTENT)
        }
        "PUT" => {
            let Some((start, total)) = request.header("content-range").and_then(|r| {
                let (range, total) = r.strip_prefix("bytes ")?.split_once('/')?;
                Some((range.split('-').next()?.parse::<u64>().ok()?, total.parse::<u64>().ok()?))
            }) else {
                return error(StatusCode::BAD_REQUEST, "invalidRange");
            };
            if start != upload.received.len() as u64 {
                return error(StatusCode::RANGE_NOT_SATISFIABLE, "invalidRange");
            }
            let last = start + request.body.len() as u64 == total;
            if !last && !request.body.len().is_multiple_of(CHUNK_MULTIPLE) {
                return error(StatusCode::BAD_REQUEST, "invalidChunkSize");
            }
            upload.received.extend_from_slice(&request.body);
            upload.total = Some(total);
            if !last {
                return Reply::json(StatusCode::ACCEPTED, &next_expected(upload));
            }
            let upload = state.uploads.remove(id).unwrap_or_else(|| unreachable!("present above"));
            let tree = &mut state.onedrive;
            if tree.child_named(&upload.parent, &upload.name, true).is_some() {
                return error(StatusCode::CONFLICT, "nameAlreadyExists");
            }
            let file = tree.add(&upload.parent, &upload.name, false, upload.received, "application/octet-stream");
            tree.get(&file).map_or_else(not_found, |item| Reply::json(StatusCode::CREATED, &resource(tree, item)))
        }
        _ => Reply::empty(StatusCode::METHOD_NOT_ALLOWED),
    }
}

/// Copy monitors answer at once: the mock copies synchronously.
pub fn monitor(state: &State, id: &str) -> Reply {
    state
        .uploads
        .get(id)
        .map_or_else(not_found, |entry| Reply::ok(&serde_json::from_str::<Value>(&entry.parent).unwrap_or(Value::Null)))
}
