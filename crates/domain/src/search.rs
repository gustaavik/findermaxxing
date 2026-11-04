//! Search criteria and the rule deciding whether an item matches.

use std::time::Duration;

use crate::category::FileCategory;
use crate::entry::FileEntry;
use crate::path::StoragePath;
use crate::units::{ByteSize, Timestamp};

const KILOBYTE: u64 = 1000;
const MEGABYTE: u64 = KILOBYTE * 1000;
const GIGABYTE: u64 = MEGABYTE * 1000;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// "Modified within the last …" choices offered to users.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModifiedWithin {
    #[default]
    Any,
    Day,
    Week,
    Month,
    Year,
}

impl ModifiedWithin {
    /// The earliest matching modification time, given the current time.
    #[must_use]
    pub fn since(self, now: Timestamp) -> Option<Timestamp> {
        let span = match self {
            Self::Any => return None,
            Self::Day => DAY,
            Self::Week => DAY * 7,
            Self::Month => DAY * 30,
            Self::Year => DAY * 365,
        };
        let span_millis = i64::try_from(span.as_millis()).unwrap_or(i64::MAX);
        Some(Timestamp::from_unix_millis(now.unix_millis().saturating_sub(span_millis)))
    }
}

/// Size choices offered to users, as half-open ranges `[min, max)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeClass {
    #[default]
    Any,
    /// Under 100 KB.
    Small,
    /// 100 KB to 10 MB.
    Medium,
    /// 10 MB to 1 GB.
    Large,
    /// 1 GB and more.
    Huge,
}

impl SizeClass {
    fn contains(self, size: ByteSize) -> bool {
        let (min, max) = match self {
            Self::Any => return true,
            Self::Small => (0, 100 * KILOBYTE),
            Self::Medium => (100 * KILOBYTE, 10 * MEGABYTE),
            Self::Large => (10 * MEGABYTE, GIGABYTE),
            Self::Huge => (GIGABYTE, u64::MAX),
        };
        (min..max).contains(&size.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HiddenItems {
    #[default]
    Exclude,
    Include,
}

/// What to look for. Every criterion must hold (logical AND).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchQuery {
    /// Whitespace-separated words; each must appear in the name, ignoring case.
    pub text: String,
    pub category: Option<FileCategory>,
    pub modified: ModifiedWithin,
    pub size: SizeClass,
    pub hidden: HiddenItems,
}

/// A query resolved against a point in time, ready to test entries.
#[derive(Debug, Clone)]
pub struct SearchMatcher {
    words: Vec<String>,
    category: Option<FileCategory>,
    modified_since: Option<Timestamp>,
    size: SizeClass,
    hidden: HiddenItems,
}

impl SearchQuery {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
            && self.category.is_none()
            && self.modified == ModifiedWithin::Any
            && self.size == SizeClass::Any
    }

    #[must_use]
    pub fn matcher(&self, now: Timestamp) -> SearchMatcher {
        SearchMatcher {
            words: self.text.split_whitespace().map(str::to_lowercase).collect(),
            category: self.category,
            modified_since: self.modified.since(now),
            size: self.size,
            hidden: self.hidden,
        }
    }
}

impl SearchMatcher {
    #[must_use]
    pub fn matches(&self, entry: &FileEntry) -> bool {
        self.hidden_ok(entry)
            && self.name_ok(entry)
            && self.category_ok(entry)
            && self.date_ok(entry)
            && self.size_ok(entry)
    }

    /// Whether to look inside a folder at all (hidden folders are skipped unless asked for).
    #[must_use]
    pub fn descends_into(&self, folder: &FileEntry) -> bool {
        self.hidden_ok(folder)
    }

    fn hidden_ok(&self, entry: &FileEntry) -> bool {
        self.hidden == HiddenItems::Include || !entry.is_hidden()
    }

    fn name_ok(&self, entry: &FileEntry) -> bool {
        if self.words.is_empty() {
            return true;
        }
        let name = entry.name().display_name().to_lowercase();
        self.words.iter().all(|word| name.contains(word.as_str()))
    }

    fn category_ok(&self, entry: &FileEntry) -> bool {
        self.category.is_none_or(|wanted| FileCategory::of(entry) == wanted)
    }

    fn date_ok(&self, entry: &FileEntry) -> bool {
        self.modified_since.is_none_or(|since| entry.metadata().modified.is_some_and(|modified| modified >= since))
    }

    fn size_ok(&self, entry: &FileEntry) -> bool {
        self.size == SizeClass::Any || entry.metadata().size.is_some_and(|size| self.size.contains(size))
    }
}

/// Identifier assigned by the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SavedSearchId(pub i64);

/// A named query over a scope folder, kept across sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSearch {
    pub id: SavedSearchId,
    pub name: String,
    pub scope: StoragePath,
    pub query: SearchQuery,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryKind, EntryMetadata, PathSegment, Visibility};

    const NOW: Timestamp = Timestamp::from_unix_millis(1_000_000_000_000);

    fn entry(name: &str, size: u64, age_days: i64, visibility: Visibility) -> FileEntry {
        let parent = StoragePath::parse("file:///d").unwrap();
        let day_ms = 24 * 60 * 60 * 1000;
        let metadata = EntryMetadata {
            size: Some(ByteSize(size)),
            modified: Some(Timestamp::from_unix_millis(NOW.unix_millis() - age_days * day_ms)),
            visibility,
            permissions: None,
        };
        FileEntry::new(&parent, PathSegment::new(name).unwrap(), EntryKind::File, metadata)
    }

    fn matches(query: &SearchQuery, entry: &FileEntry) -> bool {
        query.matcher(NOW).matches(entry)
    }

    #[test]
    fn every_word_must_appear_ignoring_case() {
        let query = SearchQuery { text: "inv 2024".into(), ..SearchQuery::default() };
        assert!(matches(&query, &entry("Invoice-2024-03.pdf", 1, 1, Visibility::Visible)));
        assert!(!matches(&query, &entry("Invoice-2023.pdf", 1, 1, Visibility::Visible)));
    }

    #[test]
    fn category_date_and_size_combine() {
        let query = SearchQuery {
            category: Some(FileCategory::Image),
            modified: ModifiedWithin::Week,
            size: SizeClass::Large,
            ..SearchQuery::default()
        };
        assert!(matches(&query, &entry("a.jpg", 20 * MEGABYTE, 3, Visibility::Visible)));
        assert!(!matches(&query, &entry("a.jpg", 20 * MEGABYTE, 10, Visibility::Visible)), "too old");
        assert!(!matches(&query, &entry("a.jpg", KILOBYTE, 3, Visibility::Visible)), "too small");
        assert!(!matches(&query, &entry("a.txt", 20 * MEGABYTE, 3, Visibility::Visible)), "wrong kind");
    }

    #[test]
    fn hidden_items_need_opting_in() {
        let hidden = entry(".secret", 1, 1, Visibility::Hidden);
        assert!(!matches(&SearchQuery::default(), &hidden));
        assert!(matches(&SearchQuery { hidden: HiddenItems::Include, ..SearchQuery::default() }, &hidden));
    }

    #[test]
    fn size_classes_are_half_open() {
        assert!(SizeClass::Small.contains(ByteSize(99_999)));
        assert!(!SizeClass::Small.contains(ByteSize(100_000)));
        assert!(SizeClass::Medium.contains(ByteSize(100_000)));
    }
}
