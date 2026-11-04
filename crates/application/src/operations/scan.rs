use domain::{ByteSize, EntryKind, FileEntry, VfsError};
use futures::TryStreamExt;

use crate::cancel::CancelToken;
use crate::ports::DynLister;

/// A source item and everything below it, in pre-order (each folder before
/// its contents), which is exactly the order a copy must create them in.
#[derive(Debug)]
pub(crate) struct ScannedTree {
    pub root: FileEntry,
    pub items: Vec<FileEntry>,
    pub bytes: ByteSize,
}

impl ScannedTree {
    pub(crate) fn item_count(&self) -> u64 {
        self.items.len() as u64
    }
}

/// Only real directories are descended; symlinks to folders are copied as links.
pub(crate) async fn scan(
    lister: &dyn DynLister,
    root: FileEntry,
    cancel: &CancelToken,
) -> Result<ScannedTree, VfsError> {
    let mut items = Vec::new();
    let mut bytes = 0_u64;
    let mut pending = vec![root.clone()];
    while let Some(entry) = pending.pop() {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        bytes += entry.metadata().size.map_or(0, |size| size.0);
        if entry.kind() == EntryKind::Directory {
            let batches: Vec<Vec<FileEntry>> = lister.list_boxed(entry.path(), cancel.clone()).try_collect().await?;
            let mut children: Vec<FileEntry> = batches.into_iter().flatten().collect();
            children.sort_by(|a, b| b.name().cmp(a.name()));
            pending.extend(children);
        }
        items.push(entry);
    }
    Ok(ScannedTree { root, items, bytes: ByteSize(bytes) })
}
