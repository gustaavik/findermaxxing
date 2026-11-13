//! What the cloud model is told, the shape of its answer, and reading it.

use application::ports::{ModelError, ModelProposal, OrganizeRequest, ProposedGroup, ProposedRename, ProposedTags};
use serde_json::{Value, json};

/// The task, the same for every provider.
pub(crate) const INSTRUCTIONS: &str = "\
You help someone tidy one folder on their computer. You get the folder's name and a list of its items: \
each has an id, its name, its kind, its size, the date it was last changed and, for documents, the start \
of its text. Propose changes they can accept or reject one by one:

- groups: new subfolders for items that clearly belong together (at least three items, by topic, project, \
or purpose). Use short folder names a person would choose, in the language of the items. Leave items \
that don't clearly belong anywhere where they are; don't sort everything.
- renames: better names only for items whose names say nothing (\"scan0001\", \"Untitled\", \"IMG_2231\") \
or mislead. Keep the meaning and the language; the extension is kept for you.
- tags: one to three short tags for documents whose content makes a tag obviously useful \
(\"Invoice\", \"Tax\", \"Contract\").

Only use the ids you were given. When the folder is already tidy, return empty lists.";

/// The answer's JSON schema (structured outputs: every object closed and
/// every field required).
#[must_use]
pub fn proposal_schema() -> Value {
    let ids = json!({"type": "array", "items": {"type": "integer"}});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["groups", "renames", "tags"],
        "properties": {
            "groups": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["folder", "item_ids", "reason"],
                "properties": {"folder": {"type": "string"}, "item_ids": ids, "reason": {"type": "string"}},
            }},
            "renames": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["item_id", "name", "reason"],
                "properties": {"item_id": {"type": "integer"}, "name": {"type": "string"}, "reason": {"type": "string"}},
            }},
            "tags": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["item_id", "tags"],
                "properties": {"item_id": {"type": "integer"}, "tags": {"type": "array", "items": {"type": "string"}}},
            }},
        },
    })
}

/// The request as the model sees it. Absent values are left out.
#[must_use]
pub fn request_json(request: &OrganizeRequest) -> Value {
    let items: Vec<Value> = request
        .items
        .iter()
        .map(|item| {
            let mut value = json!({"id": item.id, "name": item.name, "kind": item.kind});
            if let Some(size) = item.size {
                value["size"] = json!(size);
            }
            if let Some(modified) = &item.modified {
                value["modified"] = json!(modified);
            }
            if let Some(excerpt) = &item.excerpt {
                value["excerpt"] = json!(excerpt);
            }
            value
        })
        .collect();
    json!({"folder": request.folder_name, "items": items})
}

/// Reads an answer; tolerates a code fence around the JSON (some
/// OpenAI-compatible servers add one). Missing lists count as empty.
///
/// # Errors
/// [`ModelError::BadAnswer`] when there's no JSON object in `text`.
pub fn parse_proposal(text: &str) -> Result<ModelProposal, ModelError> {
    let start = text.find('{').ok_or_else(|| ModelError::BadAnswer("no JSON in the answer".into()))?;
    let end = text.rfind('}').ok_or_else(|| ModelError::BadAnswer("no JSON in the answer".into()))?;
    let value: Value = serde_json::from_str(&text[start..=end]).map_err(|e| ModelError::BadAnswer(e.to_string()))?;
    let list = |key: &str| value[key].as_array().cloned().unwrap_or_default();
    let id = |v: &Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
    let text_of = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    Ok(ModelProposal {
        groups: list("groups")
            .iter()
            .map(|g| ProposedGroup {
                folder: text_of(&g["folder"]),
                item_ids: g["item_ids"].as_array().map(|ids| ids.iter().filter_map(id).collect()).unwrap_or_default(),
                reason: text_of(&g["reason"]),
            })
            .collect(),
        renames: list("renames")
            .iter()
            .filter_map(|r| {
                Some(ProposedRename {
                    item_id: id(&r["item_id"])?,
                    name: text_of(&r["name"]),
                    reason: text_of(&r["reason"]),
                })
            })
            .collect(),
        tags: list("tags")
            .iter()
            .filter_map(|t| {
                let tags = t["tags"].as_array()?.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
                Some(ProposedTags { item_id: id(&t["item_id"])?, tags })
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_parse_with_or_without_a_fence() {
        let text = "```json\n{\"groups\":[{\"folder\":\"Taxes\",\"item_ids\":[1,2,3],\"reason\":\"Tax papers\"}],\
                    \"renames\":[{\"item_id\":4,\"name\":\"Lease\",\"reason\":\"\"}],\"tags\":[]}\n```";
        let proposal = parse_proposal(text).unwrap_or_default();
        assert_eq!(proposal.groups[0].item_ids, [1, 2, 3]);
        assert_eq!(proposal.renames[0].name, "Lease");
        assert!(parse_proposal("I can't help with that").is_err());
        assert_eq!(parse_proposal("{}").unwrap_or_default(), ModelProposal::default());
    }
}
