//! What analysing a folder starts from: its items, the names already
//! taken, and the text of its documents. Also the summary a cloud model
//! gets of it, built here so the preview and the request can't differ.

use std::collections::{HashMap, HashSet};

use domain::{
    ContentKind, EntryKind, FileCategory, FileEntry, IndexDecision, PathSegment, QueryClock, StoragePath, VfsError,
    index_decision,
};
use futures::TryStreamExt;

use crate::cancel::CancelToken;
use crate::operations::progress::ProgressTracker;
use crate::ports::{ContentExtractor, ItemSummary, MountResolver, OrganizeRequest};
use crate::use_cases::utilities::local_copy;

/// Documents read per analysis (the most recently changed first).
pub(crate) const MAX_DOCUMENTS: usize = 200;
/// Text kept per document: enough for keywords, a title and an excerpt.
const TEXT_KEPT_CHARS: usize = 4000;
/// Items described to a cloud model per request.
pub(crate) const MAX_CLOUD_ITEMS: usize = 200;
/// Characters of a document's text a cloud model may see (ADR 0029).
pub const EXCERPT_CHARS: usize = 400;

pub(crate) struct Inventory {
    pub folder: StoragePath,
    /// Visible items directly in the folder.
    pub entries: Vec<FileEntry>,
    /// Every name in the folder, hidden ones too.
    pub existing: HashSet<PathSegment>,
    /// Extracted text of the documents read.
    pub texts: HashMap<StoragePath, String>,
}

impl Inventory {
    pub(crate) fn documents(&self) -> impl Iterator<Item = (&FileEntry, &str)> {
        self.entries.iter().filter_map(|e| self.texts.get(e.path()).map(|t| (e, t.as_str())))
    }
}

pub(crate) async fn take_inventory(
    mounts: &dyn MountResolver,
    extractor: &dyn ContentExtractor,
    folder: &StoragePath,
    tracker: &ProgressTracker,
    cancel: &CancelToken,
) -> Result<Inventory, VfsError> {
    let lister = mounts.resolve(folder.mount())?.lister()?;
    let all: Vec<FileEntry> = lister.list_boxed(folder, cancel.clone()).try_collect::<Vec<_>>().await?.concat();
    let existing = all.iter().map(|e| e.name().clone()).collect();
    let entries: Vec<FileEntry> = all.into_iter().filter(|e| !e.is_hidden()).collect();
    let mut documents: Vec<&FileEntry> =
        entries.iter().filter(|e| index_decision(e, false) == IndexDecision::Index(ContentKind::Text)).collect();
    documents.sort_by_key(|e| std::cmp::Reverse(e.metadata().modified));
    documents.truncate(MAX_DOCUMENTS);
    tracker.begin(documents.len() as u64, domain::ByteSize(0));
    let mut texts = HashMap::new();
    for document in documents {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        tracker.item_started(Some(document.name()));
        if let Ok(local) = local_copy(mounts, document.path()).await
            && let Ok(Some(text)) = extractor.extract(&local, FileCategory::of(document)).await
        {
            texts.insert(document.path().clone(), text.chars().take(TEXT_KEPT_CHARS).collect());
        }
        tracker.item_finished();
    }
    Ok(Inventory { folder: folder.clone(), entries, existing, texts })
}

fn category_name(entry: &FileEntry) -> &'static str {
    match FileCategory::of(entry) {
        FileCategory::Folder => "folder",
        FileCategory::Image => "image",
        FileCategory::Pdf => "pdf",
        FileCategory::Video => "video",
        FileCategory::Audio => "audio",
        FileCategory::Text => "text",
        FileCategory::Archive => "archive",
        FileCategory::Document => "document",
        FileCategory::Other if entry.kind() == EntryKind::File => "file",
        FileCategory::Other => "other",
    }
}

/// The start of a document's text, whitespace collapsed.
fn excerpt(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let excerpt: String = collapsed.chars().take(EXCERPT_CHARS).collect();
    (!excerpt.is_empty()).then_some(excerpt)
}

/// What a cloud model would be sent for this folder, and which item each
/// id stands for. Names, kinds, sizes, dates and short excerpts only.
pub(crate) fn cloud_request(inventory: &Inventory, clock: QueryClock) -> (OrganizeRequest, Vec<FileEntry>) {
    let mut items: Vec<&FileEntry> = inventory.entries.iter().collect();
    items.sort_by_key(|e| std::cmp::Reverse(e.metadata().modified));
    items.truncate(MAX_CLOUD_ITEMS);
    let summaries = (0_u32..)
        .zip(&items)
        .map(|(id, entry)| ItemSummary {
            id,
            name: entry.name().display_name().into_owned(),
            kind: category_name(entry).to_owned(),
            size: entry.metadata().size.map(|s| s.0).filter(|_| entry.kind() == EntryKind::File),
            modified: entry.metadata().modified.map(|m| {
                let t = m.civil(clock.utc_offset_minutes);
                format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)
            }),
            excerpt: inventory.texts.get(entry.path()).and_then(|t| excerpt(t)),
        })
        .collect();
    let folder_name = inventory.folder.name().map(|n| n.display_name().into_owned()).unwrap_or_default();
    (OrganizeRequest { folder_name, items: summaries }, items.into_iter().cloned().collect())
}
