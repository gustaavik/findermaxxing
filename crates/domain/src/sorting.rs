use std::cmp::Ordering;

use crate::entry::FileEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FolderPlacement {
    #[default]
    First,
    Mixed,
}

/// How a directory listing is ordered. Ties on the primary key fall back to
/// natural name order so the result is always total and stable across reloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SortSpec {
    pub key: SortKey,
    pub order: SortOrder,
    pub folders: FolderPlacement,
}

impl SortSpec {
    #[must_use]
    pub fn compare(&self, a: &FileEntry, b: &FileEntry) -> Ordering {
        self.folder_ordering(a, b).then_with(|| self.directed(self.primary(a, b).then_with(|| by_name(a, b))))
    }

    fn folder_ordering(&self, a: &FileEntry, b: &FileEntry) -> Ordering {
        match self.folders {
            FolderPlacement::First => b.kind().is_container().cmp(&a.kind().is_container()),
            FolderPlacement::Mixed => Ordering::Equal,
        }
    }

    fn primary(&self, a: &FileEntry, b: &FileEntry) -> Ordering {
        match self.key {
            SortKey::Name => Ordering::Equal,
            SortKey::Size => a.metadata().size.cmp(&b.metadata().size),
            SortKey::Modified => a.metadata().modified.cmp(&b.metadata().modified),
            SortKey::Kind => extension(a).cmp(&extension(b)),
        }
    }

    fn directed(&self, ordering: Ordering) -> Ordering {
        match self.order {
            SortOrder::Ascending => ordering,
            SortOrder::Descending => ordering.reverse(),
        }
    }
}

fn by_name(a: &FileEntry, b: &FileEntry) -> Ordering {
    natural_cmp(a.name().as_str(), b.name().as_str())
}

fn extension(entry: &FileEntry) -> String {
    entry.name().as_str().rsplit_once('.').map(|(_, ext)| ext.to_lowercase()).unwrap_or_default()
}

/// Case-insensitive "human" ordering: `file2` < `file10`. Falls back to a
/// byte comparison so distinct names never compare equal.
#[must_use]
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    natural_cmp_folded(a, b).then_with(|| a.cmp(b))
}

fn natural_cmp_folded(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) if l.is_ascii_digit() && r.is_ascii_digit() => {
                let ordering = compare_digit_runs(&take_digits(&mut left), &take_digits(&mut right));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(l), Some(r)) => {
                let ordering = l.to_lowercase().cmp(r.to_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
                left.next();
                right.next();
            }
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut digits = String::new();
    while let Some(c) = chars.next_if(char::is_ascii_digit) {
        digits.push(c);
    }
    digits
}

fn compare_digit_runs(a: &str, b: &str) -> Ordering {
    let a_trimmed = a.trim_start_matches('0');
    let b_trimmed = b.trim_start_matches('0');
    a_trimmed.len().cmp(&b_trimmed.len()).then_with(|| a_trimmed.cmp(b_trimmed)).then_with(|| a.len().cmp(&b.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ByteSize, EntryKind, EntryMetadata, PathSegment, StoragePath};

    fn entry(name: &str, kind: EntryKind, size: u64) -> FileEntry {
        let parent = StoragePath::parse("file:///tmp").unwrap();
        let metadata = EntryMetadata { size: Some(ByteSize(size)), ..EntryMetadata::default() };
        FileEntry::new(&parent, PathSegment::new(name).unwrap(), kind, metadata)
    }

    fn sorted_names(spec: SortSpec, mut entries: Vec<FileEntry>) -> Vec<String> {
        entries.sort_by(|a, b| spec.compare(a, b));
        entries.iter().map(|e| e.name().to_string()).collect()
    }

    #[test]
    fn natural_order_handles_numbers_and_case() {
        let mut names = vec!["file10", "File2", "file1", "file02", "a", "B"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["a", "B", "file1", "File2", "file02", "file10"]);
    }

    #[test]
    fn natural_cmp_is_total_for_distinct_names() {
        assert_ne!(natural_cmp("a", "A"), Ordering::Equal);
        assert_ne!(natural_cmp("1", "01"), Ordering::Equal);
    }

    #[test]
    fn folders_first_then_name() {
        let entries = vec![
            entry("b.txt", EntryKind::File, 1),
            entry("z-dir", EntryKind::Directory, 0),
            entry("a.txt", EntryKind::File, 1),
        ];
        assert_eq!(sorted_names(SortSpec::default(), entries), ["z-dir", "a.txt", "b.txt"]);
    }

    #[test]
    fn size_descending_keeps_folders_on_top() {
        let spec = SortSpec { key: SortKey::Size, order: SortOrder::Descending, folders: FolderPlacement::First };
        let entries = vec![
            entry("small", EntryKind::File, 1),
            entry("dir", EntryKind::Directory, 0),
            entry("big", EntryKind::File, 100),
        ];
        assert_eq!(sorted_names(spec, entries), ["dir", "big", "small"]);
    }
}
