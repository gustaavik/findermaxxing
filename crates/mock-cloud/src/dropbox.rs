//! Dropbox API v2: case-insensitive paths, RPC calls with JSON bodies,
//! content calls with a `Dropbox-API-Arg` header, conflicts as 409 with an
//! `error_summary`, upload sessions that report the correct offset.

use hyper::StatusCode;
use serde_json::{Value, json};

use crate::http::{Reply, Request};
use crate::tree::{Item, ROOT, Tree, rfc3339};
use crate::{State, Upload};

const API_ARG: &str = "dropbox-api-arg";
const DEFAULT_LIMIT: usize = 2000;

fn conflict(summary: &str, error: &Value) -> Reply {
    Reply::json(StatusCode::CONFLICT, &json!({"error_summary": format!("{summary}/.."), "error": error}))
}

fn lookup_error(field: &str, tag: &str) -> Reply {
    conflict(&format!("{field}/{tag}"), &json!({".tag": field, field: {".tag": tag}}))
}

/// `/A/b.txt` → `/a/b.txt`; the root is `""`.
fn normalised(path: &str) -> String {
    path.trim_end_matches('/').to_owned()
}

fn parent_and_name(path: &str) -> (String, String) {
    let path = normalised(path);
    match path.rsplit_once('/') {
        Some((parent, name)) => (parent.to_owned(), name.to_owned()),
        None => (String::new(), path),
    }
}

fn metadata(tree: &Tree, item: &Item) -> Value {
    let path = tree.path_of(&item.id);
    if item.folder {
        json!({".tag": "folder", "name": item.name, "id": format!("id:{}", item.id), "path_display": path, "path_lower": path.to_lowercase()})
    } else {
        json!({
            ".tag": "file", "name": item.name, "id": format!("id:{}", item.id),
            "path_display": path, "path_lower": path.to_lowercase(),
            "size": item.content.len(),
            "server_modified": rfc3339(item.modified_ms).split('.').next().map(|s| format!("{s}Z")).unwrap_or_default(),
            "client_modified": rfc3339(item.modified_ms).split('.').next().map(|s| format!("{s}Z")).unwrap_or_default(),
        })
    }
}

fn find(tree: &Tree, path: &str) -> Option<String> {
    tree.by_path(&normalised(path), true)
}

#[allow(clippy::too_many_lines, reason = "one arm per endpoint")]
pub fn handle(state: &mut State, rest: &str, request: &Request) -> Reply {
    match rest {
        "api/2/users/get_current_account" => {
            Reply::ok(&json!({"email": "mock@dropbox.test", "name": {"display_name": "Mock Dropboxer"}}))
        }
        "api/2/files/list_folder" => list(&state.dropbox, &request.json(), 0),
        "api/2/files/list_folder/continue" => {
            let cursor = request.json()["cursor"].as_str().unwrap_or_default().to_owned();
            let Some((path, offset)) = cursor.rsplit_once('#') else { return lookup_error("path", "not_found") };
            list(&state.dropbox, &json!({"path": path}), offset.parse().unwrap_or(0))
        }
        "api/2/files/get_metadata" => {
            let tree = &state.dropbox;
            let path = request.json()["path"].as_str().unwrap_or_default().to_owned();
            match find(tree, &path).filter(|id| id != ROOT) {
                Some(id) => tree
                    .get(&id)
                    .map_or_else(|| lookup_error("path", "not_found"), |item| Reply::ok(&metadata(tree, item))),
                None => lookup_error("path", "not_found"),
            }
        }
        "api/2/files/create_folder_v2" => create_folder(&mut state.dropbox, &request.json()),
        "api/2/files/move_v2" => relocate(&mut state.dropbox, &request.json(), true),
        "api/2/files/copy_v2" => relocate(&mut state.dropbox, &request.json(), false),
        "api/2/files/delete_v2" => {
            let tree = &mut state.dropbox;
            let path = request.json()["path"].as_str().unwrap_or_default().to_owned();
            let Some(id) = find(tree, &path).filter(|id| id != ROOT) else {
                return lookup_error("path_lookup", "not_found");
            };
            let item = tree.get(&id).cloned();
            tree.remove_tree(&id);
            item.map_or_else(
                || lookup_error("path_lookup", "not_found"),
                |item| Reply::ok(&json!({"metadata": {"name": item.name}})),
            )
        }
        "content/2/files/download" => download(&state.dropbox, request),
        "content/2/files/upload" => {
            let arg = arg(request);
            commit(&mut state.dropbox, &arg, request.body.to_vec())
        }
        "content/2/files/upload_session/start" => {
            state.next_upload += 1;
            let id = format!("dbx-{}", state.next_upload);
            let upload = Upload {
                parent: String::new(),
                name: String::new(),
                received: request.body.to_vec(),
                total: None,
                closed: false,
            };
            state.uploads.insert(id.clone(), upload);
            Reply::ok(&json!({"session_id": id}))
        }
        "content/2/files/upload_session/append_v2" => {
            append(state, &arg(request)["cursor"], request).unwrap_or_else(|reply| reply)
        }
        "content/2/files/upload_session/finish" => {
            let arg = arg(request);
            if let Err(reply) = append(state, &arg["cursor"], request) {
                return reply;
            }
            let id = arg["cursor"]["session_id"].as_str().unwrap_or_default().to_owned();
            let Some(upload) = state.uploads.remove(&id) else { return lookup_error("lookup_failed", "not_found") };
            let reply = commit(&mut state.dropbox, &arg["commit"], upload.received.clone());
            if reply.status != StatusCode::OK {
                state.uploads.insert(id, upload);
            }
            reply
        }
        _ => Reply::empty(StatusCode::NOT_FOUND),
    }
}

fn arg(request: &Request) -> Value {
    request.header(API_ARG).and_then(|text| serde_json::from_str(text).ok()).unwrap_or(Value::Null)
}

fn list(tree: &Tree, args: &Value, offset: usize) -> Reply {
    let path = args["path"].as_str().unwrap_or_default().to_owned();
    let Some(id) = find(tree, &path) else { return lookup_error("path", "not_found") };
    if tree.get(&id).is_some_and(|item| !item.folder) {
        return lookup_error("path", "not_folder");
    }
    let limit = args["limit"].as_u64().and_then(|l| usize::try_from(l).ok()).unwrap_or(DEFAULT_LIMIT);
    let children = tree.children(&id);
    let entries: Vec<Value> = children.iter().skip(offset).take(limit).map(|item| metadata(tree, item)).collect();
    let next = offset + entries.len();
    Reply::ok(&json!({"entries": entries, "cursor": format!("{path}#{next}"), "has_more": next < children.len()}))
}

fn create_folder(tree: &mut Tree, args: &Value) -> Reply {
    let path = args["path"].as_str().unwrap_or_default().to_owned();
    if find(tree, &path).is_some() {
        return conflict(
            "path/conflict/folder",
            &json!({".tag": "path", "path": {".tag": "conflict", "conflict": {".tag": "folder"}}}),
        );
    }
    let id = tree.ensure_folders(&path, true);
    tree.get(&id)
        .map_or_else(|| lookup_error("path", "not_found"), |item| Reply::ok(&json!({"metadata": metadata(tree, item)})))
}

fn relocate(tree: &mut Tree, args: &Value, moving: bool) -> Reply {
    let from = args["from_path"].as_str().unwrap_or_default().to_owned();
    let to = args["to_path"].as_str().unwrap_or_default().to_owned();
    let Some(id) = find(tree, &from).filter(|id| id != ROOT) else { return lookup_error("from_lookup", "not_found") };
    let (to_parent, to_name) = parent_and_name(&to);
    let same_item = find(tree, &to).is_some_and(|existing| existing == id);
    if find(tree, &to).is_some() && !same_item {
        return conflict("to/conflict/file", &json!({".tag": "to", "to": {".tag": "conflict"}}));
    }
    let parent = tree.ensure_folders(&to_parent, true);
    if tree.is_within(&parent, &id) {
        return conflict("cant_move_folder_into_itself", &json!({".tag": "cant_move_folder_into_itself"}));
    }
    let result = if moving {
        if let Some(item) = tree.get_mut(&id) {
            item.parent = parent;
            item.name = to_name;
        }
        Some(id)
    } else {
        tree.copy_tree(&id, &parent, &to_name)
    };
    let item = result.and_then(|id| tree.get(&id).cloned());
    item.map_or_else(
        || lookup_error("from_lookup", "not_found"),
        |item| Reply::ok(&json!({"metadata": metadata(tree, &item)})),
    )
}

fn download(tree: &Tree, request: &Request) -> Reply {
    let path = arg(request)["path"].as_str().unwrap_or_default().to_owned();
    let Some(id) = find(tree, &path).filter(|id| id != ROOT) else { return lookup_error("path", "not_found") };
    let Some(item) = tree.get(&id) else { return lookup_error("path", "not_found") };
    if item.folder {
        return lookup_error("path", "not_file");
    }
    Reply::content(&item.content, request.range_start())
        .with_header("dropbox-api-result", &metadata(tree, item).to_string())
}

/// `mode: add` without autorename: an existing name is a conflict.
fn commit(tree: &mut Tree, args: &Value, content: Vec<u8>) -> Reply {
    let path = args["path"].as_str().unwrap_or_default().to_owned();
    if find(tree, &path).is_some() {
        return conflict("path/conflict/file", &json!({".tag": "path", "reason": {".tag": "conflict"}}));
    }
    let (parent_path, name) = parent_and_name(&path);
    let parent = tree.ensure_folders(&parent_path, true);
    let id = tree.add(&parent, &name, false, content, "application/octet-stream");
    tree.get(&id).map_or_else(|| lookup_error("path", "not_found"), |item| Reply::ok(&metadata(tree, item)))
}

fn append(state: &mut State, cursor: &Value, request: &Request) -> Result<Reply, Reply> {
    let id = cursor["session_id"].as_str().unwrap_or_default();
    let offset = cursor["offset"].as_u64().unwrap_or_default();
    let Some(upload) = state.uploads.get_mut(id) else {
        return Err(conflict("not_found", &json!({".tag": "not_found"})));
    };
    if upload.closed {
        return Err(conflict("closed", &json!({".tag": "closed"})));
    }
    let correct = upload.received.len() as u64;
    if offset != correct {
        return Err(conflict("incorrect_offset", &json!({".tag": "incorrect_offset", "correct_offset": correct})));
    }
    upload.received.extend_from_slice(&request.body);
    upload.closed |= arg(request)["close"].as_bool() == Some(true);
    Ok(Reply::ok(&Value::Null))
}
