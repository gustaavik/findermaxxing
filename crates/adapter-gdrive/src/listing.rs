//! Folders by id, children by display name. Drive allows several items with
//! one name, so names shown in the app are disambiguated (`report (2).pdf`);
//! Google Docs show with the Office extension they export to.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use domain::{
    ByteSize, EntryKind, EntryMetadata, FileEntry, PathSegment, StoragePath, Timestamp, VfsError, Visibility,
    disambiguate,
};
use serde_json::Value;

pub(crate) const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const GOOGLE_APPS_PREFIX: &str = "application/vnd.google-apps.";
const HIDDEN_PREFIX: u8 = b'.';
/// How long a folder's children are trusted before asking again.
const LISTING_TTL: Duration = Duration::from_secs(3);

/// Google Docs Editors files and what they are exported as.
pub(crate) const EXPORTS: &[(&str, &str, &str)] = &[
    ("document", ".docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ("spreadsheet", ".xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    ("presentation", ".pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    ("drawing", ".png", "image/png"),
];

/// What a Drive item is to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Content {
    Folder,
    /// Bytes downloadable with `alt=media`.
    Binary,
    /// A Google Doc: exported as this MIME type; `suffix` is shown after its name.
    Export {
        mime: &'static str,
        suffix: &'static str,
    },
    /// Forms, sites, shortcuts…: listed, but there is nothing to read.
    Unreadable,
}

#[derive(Debug, Clone)]
pub(crate) struct Child {
    pub id: String,
    /// The name the app shows (with export suffix, disambiguated).
    pub name: PathSegment,
    pub content: Content,
    pub size: Option<u64>,
    pub modified: Option<Timestamp>,
}

impl Child {
    pub(crate) fn kind(&self) -> EntryKind {
        match self.content {
            Content::Folder => EntryKind::Directory,
            Content::Binary | Content::Export { .. } => EntryKind::File,
            Content::Unreadable => EntryKind::Other,
        }
    }

    pub(crate) fn entry(&self, parent: &StoragePath) -> FileEntry {
        let hidden = self.name.as_bytes().first() == Some(&HIDDEN_PREFIX);
        let metadata = EntryMetadata {
            size: (self.content == Content::Binary).then(|| ByteSize(self.size.unwrap_or_default())),
            modified: self.modified,
            visibility: if hidden { Visibility::Hidden } else { Visibility::Visible },
            permissions: None,
        };
        FileEntry::new(parent, self.name.clone(), self.kind(), metadata)
    }

    /// The Drive name for a display name given to this item (the export
    /// suffix of a Google Doc isn't part of its Drive name).
    pub(crate) fn drive_name_for(&self, display: &PathSegment) -> String {
        let display = display.display_name().into_owned();
        match &self.content {
            Content::Export { suffix, .. } => display.strip_suffix(suffix).map_or(display.clone(), str::to_owned),
            _ => display,
        }
    }
}

pub(crate) fn content_of(mime: &str) -> Content {
    if mime == FOLDER_MIME {
        return Content::Folder;
    }
    let Some(kind) = mime.strip_prefix(GOOGLE_APPS_PREFIX) else { return Content::Binary };
    EXPORTS
        .iter()
        .find(|(google, _, _)| *google == kind)
        .map_or(Content::Unreadable, |(_, suffix, mime)| Content::Export { mime, suffix })
}

/// Turns one folder's `files` (oldest first) into children with unique names.
pub(crate) fn children_of(files: &[Value]) -> Vec<Child> {
    let mut raw: Vec<Child> = files
        .iter()
        .filter_map(|file| {
            let drive_name = file["name"].as_str()?.to_owned();
            let content = content_of(file["mimeType"].as_str().unwrap_or_default());
            let shown = match &content {
                Content::Export { suffix, .. } => format!("{drive_name}{suffix}"),
                _ => drive_name,
            };
            // Names Drive allows but paths can't hold become visible look-alikes.
            let name = PathSegment::new(&shown.replace('/', "\u{2215}")).ok()?;
            Some(Child {
                id: file["id"].as_str()?.to_owned(),
                name,
                content,
                size: file["size"].as_str().and_then(|s| s.parse().ok()),
                modified: file["modifiedTime"].as_str().and_then(infra_oauth::parse_rfc3339),
            })
        })
        .collect();
    let names: Vec<(PathSegment, EntryKind)> = raw.iter().map(|c| (c.name.clone(), c.kind())).collect();
    for (child, name) in raw.iter_mut().zip(disambiguate(&names)) {
        child.name = name;
    }
    raw
}

/// Recently fetched folder listings, keyed by folder id.
#[derive(Debug, Default)]
pub(crate) struct ListingCache {
    folders: HashMap<String, (Instant, Vec<Child>)>,
}

impl ListingCache {
    pub(crate) fn get(&self, folder: &str) -> Option<Vec<Child>> {
        let (at, children) = self.folders.get(folder)?;
        (at.elapsed() < LISTING_TTL).then(|| children.clone())
    }

    pub(crate) fn put(&mut self, folder: &str, children: Vec<Child>) {
        self.folders.insert(folder.to_owned(), (Instant::now(), children));
    }

    /// After any change: names and duplicates may have shifted anywhere.
    pub(crate) fn clear(&mut self) {
        self.folders.clear();
    }
}

pub(crate) fn find<'a>(children: &'a [Child], name: &PathSegment) -> Result<&'a Child, VfsError> {
    children.iter().find(|child| child.name == *name).ok_or(VfsError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn docs_get_office_names_and_duplicates_get_numbers() {
        let files = [
            json!({"id": "1", "name": "Budget", "mimeType": "application/vnd.google-apps.spreadsheet"}),
            json!({"id": "2", "name": "report.pdf", "mimeType": "application/pdf", "size": "10"}),
            json!({"id": "3", "name": "report.pdf", "mimeType": "application/pdf", "size": "12"}),
            json!({"id": "4", "name": "Survey", "mimeType": "application/vnd.google-apps.form"}),
        ];
        let children = children_of(&files);
        let names: Vec<String> = children.iter().map(|c| c.name.display_name().into_owned()).collect();
        assert_eq!(names, ["Budget.xlsx", "report.pdf", "report (2).pdf", "Survey"]);
        assert_eq!(children[0].drive_name_for(&PathSegment::new("Plan.xlsx").unwrap()), "Plan");
        assert_eq!(children[3].kind(), EntryKind::Other);
        assert_eq!(children[2].size, Some(12));
    }
}
