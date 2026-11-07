//! Finding duplicates in stages, cheapest first: equal sizes, then a hash of
//! the first bytes, then a hash of everything. Optionally, images that look
//! alike (a perceptual hash within a small distance) form "similar" groups.

use std::collections::HashMap;
use std::hash::Hash;

use crate::entry::{EntryKind, FileEntry};
use crate::sorting::natural_cmp;
use crate::units::ByteSize;

/// Bytes read for the cheap middle stage; most different files of equal size
/// already differ here.
pub const PARTIAL_HASH_BYTES: u64 = 64 * 1024;
/// Empty files are all "identical" and say nothing.
pub const DEFAULT_MIN_DUPLICATE_SIZE: ByteSize = ByteSize(1);
/// Perceptual-hash bits that may differ for two images to count as similar
/// (out of 64): resized or re-encoded copies, not merely related photos.
pub const SIMILAR_IMAGE_DISTANCE: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateMatch {
    /// Byte-for-byte equal.
    Identical,
    /// Images that look the same (resized, recompressed, another format).
    Similar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub matching: DuplicateMatch,
    /// The suggested one to keep comes first.
    pub items: Vec<FileEntry>,
}

impl DuplicateGroup {
    /// Space freed by keeping only the first item.
    #[must_use]
    pub fn reclaimable(&self) -> ByteSize {
        ByteSize(self.items.iter().skip(1).map(|item| size_of(item).0).sum())
    }
}

/// A perceptual image hash (64 bits for the default hasher).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageFingerprint(pub Vec<u8>);

impl ImageFingerprint {
    #[must_use]
    pub fn distance(&self, other: &Self) -> u32 {
        let differing: u32 = self.0.iter().zip(&other.0).map(|(a, b)| (a ^ b).count_ones()).sum();
        let length_difference = self.0.len().abs_diff(other.0.len()) * 8;
        differing + u32::try_from(length_difference).unwrap_or(u32::MAX)
    }
}

fn size_of(entry: &FileEntry) -> ByteSize {
    entry.metadata().size.unwrap_or_default()
}

/// Files of at least `min_size` that share their size with another file.
#[must_use]
pub fn same_size_groups(entries: Vec<FileEntry>, min_size: ByteSize) -> Vec<Vec<FileEntry>> {
    let files = entries.into_iter().filter(|e| e.kind() == EntryKind::File && size_of(e) >= min_size);
    split_by(files.map(|entry| (size_of(&entry), entry)).collect())
}

/// Splits items by key, keeping only keys shared by two or more items.
#[must_use]
pub fn split_by<K: Hash + Eq>(keyed: Vec<(K, FileEntry)>) -> Vec<Vec<FileEntry>> {
    let mut groups: HashMap<K, Vec<FileEntry>> = HashMap::new();
    for (key, entry) in keyed {
        groups.entry(key).or_default().push(entry);
    }
    groups.into_values().filter(|group| group.len() > 1).collect()
}

/// Groups images whose fingerprints are within `max_distance` of each other
/// (transitively).
#[must_use]
pub fn similar_groups(fingerprinted: Vec<(ImageFingerprint, FileEntry)>, max_distance: u32) -> Vec<Vec<FileEntry>> {
    let mut parent: Vec<usize> = (0..fingerprinted.len()).collect();
    for i in 0..fingerprinted.len() {
        for j in (i + 1)..fingerprinted.len() {
            if fingerprinted[i].0.distance(&fingerprinted[j].0) <= max_distance {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[b] = a;
            }
        }
    }
    let roots: Vec<usize> = (0..fingerprinted.len()).map(|i| root(&mut parent, i)).collect();
    split_by(roots.into_iter().zip(fingerprinted).map(|(r, (_, entry))| (r, entry)).collect())
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Orders a group with the one to keep first: the oldest, then the one
/// with the shortest path (the "original" rarely sits deepest), then by name.
#[must_use]
pub fn keeper_first(matching: DuplicateMatch, mut items: Vec<FileEntry>) -> DuplicateGroup {
    items.sort_by(|a, b| {
        let age = a.metadata().modified.cmp(&b.metadata().modified);
        let modified_both = a.metadata().modified.is_some() && b.metadata().modified.is_some();
        let age = if modified_both { age } else { std::cmp::Ordering::Equal };
        age.then_with(|| a.path().segments().len().cmp(&b.path().segments().len()))
            .then_with(|| natural_cmp(&a.path().path_string(), &b.path().path_string()))
    });
    DuplicateGroup { matching, items }
}

/// Most space to win first.
pub fn sort_groups(groups: &mut [DuplicateGroup]) {
    groups.sort_by_key(|group| std::cmp::Reverse(group.reclaimable()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryMetadata, PathSegment, StoragePath, Timestamp};

    fn file(path: &str, size: u64, modified: i64) -> FileEntry {
        let path = StoragePath::parse(&format!("file://{path}")).unwrap();
        let metadata = EntryMetadata {
            size: Some(ByteSize(size)),
            modified: Some(Timestamp::from_unix_millis(modified)),
            ..EntryMetadata::default()
        };
        FileEntry::new(&path.parent().unwrap(), path.name().unwrap().clone(), EntryKind::File, metadata)
    }

    fn names(group: &[FileEntry]) -> Vec<String> {
        let mut names: Vec<String> = group.iter().map(|e| e.path().path_string()).collect();
        names.sort();
        names
    }

    #[test]
    fn only_shared_sizes_survive_the_first_stage() {
        let entries =
            vec![file("/a", 10, 0), file("/b", 10, 0), file("/c", 11, 0), file("/empty1", 0, 0), file("/empty2", 0, 0)];
        let groups = same_size_groups(entries, DEFAULT_MIN_DUPLICATE_SIZE);
        assert_eq!(groups.len(), 1);
        assert_eq!(names(&groups[0]), ["/a", "/b"]);
        let folder = FileEntry::new(
            &StoragePath::parse("file:///").unwrap(),
            PathSegment::new("dir").unwrap(),
            EntryKind::Directory,
            EntryMetadata { size: Some(ByteSize(10)), ..EntryMetadata::default() },
        );
        assert!(same_size_groups(vec![folder, file("/x", 10, 0)], ByteSize(1)).is_empty());
    }

    #[test]
    fn the_oldest_and_shallowest_copy_is_kept() {
        let group = keeper_first(
            DuplicateMatch::Identical,
            vec![file("/deep/down/copy.jpg", 5, 200), file("/photos/x.jpg", 5, 100), file("/x.jpg", 5, 100)],
        );
        assert_eq!(group.items[0].path().path_string(), "/x.jpg");
        assert_eq!(group.reclaimable(), ByteSize(10));
    }

    #[test]
    fn similar_images_group_transitively_within_the_distance() {
        let fp = |bits: u64| ImageFingerprint(bits.to_be_bytes().to_vec());
        let groups = similar_groups(
            vec![
                (fp(0b0000), file("/a", 1, 0)),
                (fp(0b0111), file("/b", 2, 0)),
                (fp(0b0111_1111), file("/c", 3, 0)),
                (fp(u64::MAX), file("/d", 4, 0)),
            ],
            4,
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(names(&groups[0]), ["/a", "/b", "/c"], "a~b (3 bits), b~c (4 bits)");
    }
}
