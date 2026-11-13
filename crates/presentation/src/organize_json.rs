//! A resolved organization plan as JSON, so the Organize window can hand
//! exactly what it previewed to the window's `OperationsController`.

use domain::{Relocation, Resolution, StoragePath, Tag};
use serde_json::{Value, json};

fn location(value: &Value) -> Option<StoragePath> {
    StoragePath::decode(value.as_str()?).ok()
}

fn locations(value: &Value) -> Vec<StoragePath> {
    value.as_array().map(|items| items.iter().filter_map(location).collect()).unwrap_or_default()
}

pub(crate) fn resolution_to_json(resolution: &Resolution) -> String {
    let encode = |paths: &[StoragePath]| paths.iter().map(StoragePath::encode).collect::<Vec<_>>();
    json!({
        "folders": encode(&resolution.folders_to_create),
        "trash": encode(&resolution.trash),
        "moves": resolution.moves.iter().map(|m| json!({"from": m.from.encode(), "to": m.to.encode()})).collect::<Vec<_>>(),
        "tags": resolution.tags.iter().map(|(path, tags)| json!({
            "path": path.encode(),
            "tags": tags.iter().map(Tag::as_str).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

/// `None` for anything that isn't such a plan.
pub(crate) fn resolution_from_json(text: &str) -> Option<Resolution> {
    let value: Value = serde_json::from_str(text).ok()?;
    let moves = value["moves"]
        .as_array()?
        .iter()
        .map(|m| Some(Relocation { from: location(&m["from"])?, to: location(&m["to"])? }))
        .collect::<Option<Vec<_>>>()?;
    let tags = value["tags"]
        .as_array()?
        .iter()
        .map(|t| {
            let tags = t["tags"].as_array()?.iter().map(|v| Tag::new(v.as_str()?).ok()).collect::<Option<Vec<_>>>()?;
            Some((location(&t["path"])?, tags))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Resolution {
        folders_to_create: locations(&value["folders"]),
        trash: locations(&value["trash"]),
        moves,
        tags,
        preview: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_round_trip() {
        let p = |s: &str| StoragePath::parse(&format!("file://{s}")).unwrap();
        let resolution = Resolution {
            folders_to_create: vec![p("/d/Invoices")],
            trash: vec![p("/d/old.dmg")],
            moves: vec![Relocation { from: p("/d/a b.pdf"), to: p("/d/Invoices/a b.pdf") }],
            tags: vec![(p("/d/Invoices/a b.pdf"), vec![Tag::new("Invoice").unwrap()])],
            preview: Vec::new(),
        };
        assert_eq!(resolution_from_json(&resolution_to_json(&resolution)), Some(resolution));
        assert_eq!(resolution_from_json("{}"), None);
    }
}
