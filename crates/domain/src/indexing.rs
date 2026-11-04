//! Rules of the content index (ADR 0025): what is indexed, how text is cut
//! into passages, how matches rank, and what the indexer reports.

use crate::category::FileCategory;
use crate::entry::FileEntry;
use crate::path::StoragePath;
use crate::units::{ByteSize, Timestamp};

/// Folder names never descended into: build output, dependencies, VCS data,
/// caches and app bundles' insides.
const SKIPPED_FOLDERS: &[&str] = &[
    "node_modules",
    ".git",
    ".hg",
    ".svn",
    "target",
    "build",
    "dist",
    "__pycache__",
    ".venv",
    "venv",
    ".cache",
    "Caches",
    "Library",
    ".Trash",
    ".npm",
    ".cargo",
    ".gradle",
    "Pods",
    "DerivedData",
];
/// Bundles look like folders but are single documents to the user.
const BUNDLE_SUFFIXES: &[&str] = &[".app", ".photoslibrary", ".bundle", ".framework", ".xcodeproj"];
/// Generated files that read as text but say nothing to the user:
/// dependency lock files and minified bundles.
const SKIPPED_FILES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "Gemfile.lock",
    "poetry.lock",
    "composer.lock",
    "Podfile.lock",
    "go.sum",
];
const SKIPPED_FILE_SUFFIXES: &[&str] = &[".lock", ".min.js", ".min.css", ".map"];
/// Text larger than this is mostly logs or data dumps; its start is enough.
pub const MAX_TEXT_FILE: ByteSize = ByteSize(20 * 1024 * 1024);
pub const MAX_IMAGE_FILE: ByteSize = ByteSize(40 * 1024 * 1024);
/// Passages of about 250 tokens (multilingual-e5 reads up to 512).
pub const PASSAGE_CHARS: usize = 1000;
pub const PASSAGE_OVERLAP: usize = 150;
/// Beyond this, a document is represented by its first passages only.
pub const MAX_PASSAGES: usize = 48;

/// Which embedding space an item lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentKind {
    Text,
    Image,
}

/// The folders to index and the ones to leave out (excludes win).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexScope {
    pub include: Vec<StoragePath>,
    pub exclude: Vec<StoragePath>,
}

impl IndexScope {
    #[must_use]
    pub fn covers(&self, location: &StoragePath) -> bool {
        self.include.iter().any(|root| location.is_within(root))
            && !self.exclude.iter().any(|ex| location.is_within(ex))
    }
}

/// What the indexer does with an item it meets while walking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexDecision {
    Descend,
    Index(ContentKind),
    Skip,
}

/// Hidden items and dependency/build folders are skipped; text-like files
/// and documents are read; images only when image search is on.
#[must_use]
pub fn index_decision(entry: &FileEntry, images: bool) -> IndexDecision {
    let name = entry.name().display_name();
    if entry.metadata().visibility == crate::entry::Visibility::Hidden {
        return IndexDecision::Skip;
    }
    let category = FileCategory::of(entry);
    if category == FileCategory::Folder {
        let bundle = BUNDLE_SUFFIXES.iter().any(|suffix| name.ends_with(suffix));
        let skipped = SKIPPED_FOLDERS.contains(&name.as_ref());
        return if bundle || skipped || !entry.kind().eq(&crate::entry::EntryKind::Directory) {
            IndexDecision::Skip
        } else {
            IndexDecision::Descend
        };
    }
    let lower = name.to_lowercase();
    if SKIPPED_FILES.iter().any(|skipped| skipped.eq_ignore_ascii_case(&name))
        || SKIPPED_FILE_SUFFIXES.iter().any(|suffix| lower.ends_with(suffix))
    {
        return IndexDecision::Skip;
    }
    let size = entry.metadata().size.unwrap_or_default();
    match category {
        FileCategory::Text | FileCategory::Pdf | FileCategory::Document if size <= MAX_TEXT_FILE => {
            IndexDecision::Index(ContentKind::Text)
        }
        FileCategory::Image if images && size <= MAX_IMAGE_FILE && !lower.ends_with(".svg") => {
            IndexDecision::Index(ContentKind::Image)
        }
        _ => IndexDecision::Skip,
    }
}

/// Cuts text into overlapping passages at whitespace, collapsing runs of
/// whitespace first. Empty text has no passages.
#[must_use]
pub fn passages(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut passages = Vec::new();
    let mut start = 0;
    while start < words.len() && passages.len() < MAX_PASSAGES {
        let mut end = start;
        let mut length = 0;
        while end < words.len() && (length == 0 || length + words[end].len() < PASSAGE_CHARS) {
            length += words[end].len() + 1;
            end += 1;
        }
        passages.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        // Step back so consecutive passages share some context.
        let mut back = end;
        let mut overlap = 0;
        while back > start + 1 && overlap < PASSAGE_OVERLAP {
            back -= 1;
            overlap += words[back].len() + 1;
        }
        start = back.max(start + 1);
    }
    passages
}

/// The version of an item that was indexed; a change means re-indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub size: Option<ByteSize>,
    pub modified: Option<Timestamp>,
}

impl Fingerprint {
    #[must_use]
    pub fn of(entry: &FileEntry) -> Self {
        Self { size: entry.metadata().size, modified: entry.metadata().modified }
    }
}

/// What the indexer is doing, for the status line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IndexStatus {
    /// AI search is turned off.
    #[default]
    Off,
    /// Turned on, but the model isn't downloaded yet.
    NeedsModel,
    Downloading {
        done: ByteSize,
        total: ByteSize,
    },
    /// Looking for new, changed and removed items.
    Scanning {
        seen: u64,
    },
    Indexing {
        done: u64,
        total: u64,
        current: String,
    },
    Paused {
        indexed: u64,
    },
    UpToDate {
        indexed: u64,
    },
    Failed(String),
}

/// Whether an indexed item of `actual` category answers a query asking for
/// `wanted`. To people, "documents" are anything they read: notes and PDFs
/// as much as office files.
#[must_use]
pub fn category_matches(wanted: FileCategory, actual: FileCategory) -> bool {
    match wanted {
        FileCategory::Document => matches!(actual, FileCategory::Document | FileCategory::Text | FileCategory::Pdf),
        _ => wanted == actual,
    }
}

/// Relevance of a document for a query: its best passage, nudged up when
/// the name itself contains query words (people often remember names).
#[must_use]
pub fn relevance(best_similarity: f32, name: &str, query: &str) -> f32 {
    const NAME_BOOST: f32 = 0.03;
    let name = name.to_lowercase();
    let hits = query.split_whitespace().filter(|w| w.chars().count() > 2 && name.contains(&w.to_lowercase())).count();
    best_similarity + NAME_BOOST * f32::from(u8::try_from(hits.min(3)).unwrap_or(3))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{EntryKind, EntryMetadata, Visibility};
    use crate::path::PathSegment;

    fn entry(name: &str, kind: EntryKind, size: u64) -> FileEntry {
        let parent = StoragePath::parse("file:///Users/me").unwrap();
        let hidden = if name.starts_with('.') { Visibility::Hidden } else { Visibility::Visible };
        let metadata = EntryMetadata { size: Some(ByteSize(size)), visibility: hidden, ..EntryMetadata::default() };
        FileEntry::new(&parent, PathSegment::new(name).unwrap(), kind, metadata)
    }

    #[test]
    fn decides_what_to_read() {
        let file = EntryKind::File;
        assert_eq!(index_decision(&entry("notes.md", file, 10), false), IndexDecision::Index(ContentKind::Text));
        assert_eq!(index_decision(&entry("report.pdf", file, 10), false), IndexDecision::Index(ContentKind::Text));
        assert_eq!(index_decision(&entry("beach.jpg", file, 10), false), IndexDecision::Skip, "images are opt-in");
        assert_eq!(index_decision(&entry("beach.jpg", file, 10), true), IndexDecision::Index(ContentKind::Image));
        assert_eq!(index_decision(&entry("huge.log", file, MAX_TEXT_FILE.0 + 1), false), IndexDecision::Skip);
        assert_eq!(index_decision(&entry("movie.mp4", file, 10), true), IndexDecision::Skip);
        assert_eq!(index_decision(&entry(".env", file, 10), false), IndexDecision::Skip);
        for generated in ["package-lock.json", "Cargo.lock", "flake.lock", "app.min.js", "app.js.map"] {
            assert_eq!(index_decision(&entry(generated, file, 10), false), IndexDecision::Skip, "{generated}");
        }
        assert_eq!(index_decision(&entry("package.json", file, 10), false), IndexDecision::Index(ContentKind::Text));
        let folder = EntryKind::Directory;
        assert_eq!(index_decision(&entry("Projects", folder, 0), false), IndexDecision::Descend);
        assert_eq!(index_decision(&entry("node_modules", folder, 0), false), IndexDecision::Skip);
        assert_eq!(index_decision(&entry("Safari.app", folder, 0), false), IndexDecision::Skip);
    }

    #[test]
    fn passages_overlap_and_are_bounded() {
        assert!(passages("   ").is_empty());
        assert_eq!(passages("short   text\n here"), ["short text here"]);
        let long: String = (0..2000).map(|i| format!("word{i} ")).collect();
        let cut = passages(&long);
        assert!(cut.iter().all(|p| p.len() <= PASSAGE_CHARS));
        assert!(cut.len() > 1 && cut.len() <= MAX_PASSAGES);
        let last_of_first = cut[0].split(' ').next_back().unwrap();
        assert!(cut[1].contains(last_of_first), "consecutive passages overlap");
    }

    #[test]
    fn scopes_include_and_exclude() {
        let path = |p: &str| StoragePath::parse(&format!("file://{p}")).unwrap();
        let scope = IndexScope {
            include: vec![path("/Users/me/Documents")],
            exclude: vec![path("/Users/me/Documents/Private")],
        };
        assert!(scope.covers(&path("/Users/me/Documents/a.txt")));
        assert!(!scope.covers(&path("/Users/me/Documents/Private/b.txt")));
        assert!(!scope.covers(&path("/Users/me/Desktop/c.txt")));
    }

    #[test]
    fn documents_include_notes_and_pdfs() {
        assert!(category_matches(FileCategory::Document, FileCategory::Text));
        assert!(category_matches(FileCategory::Document, FileCategory::Pdf));
        assert!(!category_matches(FileCategory::Pdf, FileCategory::Document), "asking for PDFs means PDFs");
        assert!(!category_matches(FileCategory::Document, FileCategory::Image));
    }

    #[test]
    fn names_nudge_relevance() {
        assert!(relevance(0.80, "Invoice 2026-03.pdf", "invoice march") > relevance(0.80, "scan.pdf", "invoice march"));
        assert!((relevance(0.8, "x", "an") - 0.8).abs() < f32::EPSILON, "short words don't count");
    }
}
