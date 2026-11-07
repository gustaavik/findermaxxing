//! What takes up space below a folder, as a tree sized for a treemap: every
//! folder, its largest files individually, and the rest of its files as one
//! "smaller files" item, so a million files don't become a million nodes.

use crate::entry::{EntryKind, FileEntry};
use crate::path::StoragePath;
use crate::units::ByteSize;

/// Files per folder shown on their own; the rest are summed.
pub const LARGEST_FILES_SHOWN: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageKind {
    Folder,
    File,
    /// The files of a folder too small to show individually.
    SmallerFiles,
    /// A folder that couldn't be read; its size is unknown (counted as 0).
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageNode {
    /// For [`UsageKind::SmallerFiles`], the folder they are in.
    pub path: StoragePath,
    pub kind: UsageKind,
    pub size: ByteSize,
    /// Files below (1 for a file).
    pub files: u64,
    /// Largest first.
    pub children: Vec<UsageNode>,
}

impl UsageNode {
    #[must_use]
    pub fn file(entry: &FileEntry) -> Self {
        Self {
            path: entry.path().clone(),
            kind: UsageKind::File,
            size: entry.metadata().size.unwrap_or_default(),
            files: 1,
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn unreadable(path: StoragePath) -> Self {
        Self { path, kind: UsageKind::Unreadable, size: ByteSize(0), files: 0, children: Vec::new() }
    }

    /// A folder from its direct files (folders among them are ignored: they
    /// come as `subfolders`) and its already-measured subfolders.
    #[must_use]
    pub fn folder(path: StoragePath, entries: &[FileEntry], subfolders: Vec<Self>) -> Self {
        let mut files: Vec<Self> =
            entries.iter().filter(|e| !matches!(e.kind(), EntryKind::Directory)).map(Self::file).collect();
        files.sort_by_key(|file| std::cmp::Reverse(file.size));
        let rest = if files.len() > LARGEST_FILES_SHOWN { files.split_off(LARGEST_FILES_SHOWN) } else { Vec::new() };
        let mut children = subfolders;
        children.extend(files);
        if !rest.is_empty() {
            children.push(Self {
                path: path.clone(),
                kind: UsageKind::SmallerFiles,
                size: ByteSize(rest.iter().map(|f| f.size.0).sum()),
                files: rest.len() as u64,
                children: Vec::new(),
            });
        }
        children.sort_by_key(|child| std::cmp::Reverse(child.size));
        let size = ByteSize(children.iter().map(|c| c.size.0).sum());
        let files = children.iter().map(|c| c.files).sum();
        Self { path, kind: UsageKind::Folder, size, files, children }
    }

    /// The folder node at `path` within this tree.
    #[must_use]
    pub fn find(&self, path: &StoragePath) -> Option<&Self> {
        if self.path == *path && self.kind == UsageKind::Folder {
            return Some(self);
        }
        if !path.is_within(&self.path) {
            return None;
        }
        self.children.iter().filter(|c| c.kind == UsageKind::Folder).find_map(|c| c.find(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryMetadata, PathSegment};

    fn p(path: &str) -> StoragePath {
        StoragePath::parse(&format!("file://{path}")).unwrap()
    }

    fn file(parent: &str, name: &str, size: u64) -> FileEntry {
        let metadata = EntryMetadata { size: Some(ByteSize(size)), ..EntryMetadata::default() };
        FileEntry::new(&p(parent), PathSegment::new(name).unwrap(), EntryKind::File, metadata)
    }

    #[test]
    fn folders_sum_their_children_largest_first() {
        let sub = UsageNode::folder(p("/a/sub"), &[file("/a/sub", "big", 500)], Vec::new());
        let top = UsageNode::folder(p("/a"), &[file("/a", "x", 10), file("/a", "y", 700)], vec![sub]);
        assert_eq!(top.size, ByteSize(1210));
        assert_eq!(top.files, 3);
        let sizes: Vec<u64> = top.children.iter().map(|c| c.size.0).collect();
        assert_eq!(sizes, [700, 500, 10]);
        assert_eq!(top.find(&p("/a/sub")).map(|n| n.size), Some(ByteSize(500)));
        assert_eq!(top.find(&p("/b")), None);
    }

    #[test]
    fn many_small_files_become_one_item() {
        let entries: Vec<FileEntry> = (0..100).map(|i| file("/m", &format!("f{i}"), 1000 + i)).collect();
        let node = UsageNode::folder(p("/m"), &entries, Vec::new());
        assert_eq!(node.children.len(), LARGEST_FILES_SHOWN + 1);
        let rest = node.children.iter().find(|c| c.kind == UsageKind::SmallerFiles).unwrap();
        assert_eq!(rest.files, 60);
        assert_eq!(node.files, 100);
        assert_eq!(node.size.0, (0..100).map(|i| 1000 + i).sum::<u64>());
    }
}
