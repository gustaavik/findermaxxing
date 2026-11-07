//! Measures what takes up space below a folder, on any provider. Symlinks
//! are counted as themselves, never followed; unreadable folders are kept
//! as such so the user sees what couldn't be measured.

use std::sync::Arc;

use domain::{EntryKind, FileEntry, StoragePath, UsageNode, VfsError};
use futures::TryStreamExt;

use super::checksums::opaque_debug;
use crate::cancel::CancelToken;
use crate::ports::{DynLister, MountResolver};

/// Progress of a measurement: files counted so far and the folder being read.
pub type UsageProgress = Arc<dyn Fn(u64, &StoragePath) + Send + Sync>;

pub struct MeasureDiskUsage {
    mounts: Arc<dyn MountResolver>,
}

/// A folder being measured: its own entries, and its subfolders measured so far.
struct Frame {
    path: StoragePath,
    entries: Vec<FileEntry>,
    pending: Vec<StoragePath>,
    measured: Vec<UsageNode>,
}

impl MeasureDiskUsage {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// # Errors
    /// `Cancelled`, or `folder` itself can't be listed.
    pub async fn execute(
        &self,
        folder: &StoragePath,
        progress: UsageProgress,
        cancel: CancelToken,
    ) -> Result<UsageNode, VfsError> {
        let lister = self.mounts.resolve(folder.mount())?.lister()?;
        let mut counted = 0_u64;
        let root = open_frame(&*lister, folder.clone(), &cancel).await?;
        counted += files_in(&root);
        let mut stack = vec![root];
        loop {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            let Some(top) = stack.last_mut() else { return Err(VfsError::NotFound) };
            if let Some(next) = top.pending.pop() {
                progress(counted, &next);
                match open_frame(&*lister, next.clone(), &cancel).await {
                    Ok(frame) => {
                        counted += files_in(&frame);
                        stack.push(frame);
                    }
                    Err(VfsError::Cancelled) => return Err(VfsError::Cancelled),
                    Err(_) => top.measured.push(UsageNode::unreadable(next)),
                }
                continue;
            }
            let Some(done) = stack.pop() else { return Err(VfsError::NotFound) };
            let node = UsageNode::folder(done.path, &done.entries, done.measured);
            match stack.last_mut() {
                Some(parent) => parent.measured.push(node),
                None => return Ok(node),
            }
        }
    }
}

async fn open_frame(lister: &dyn DynLister, path: StoragePath, cancel: &CancelToken) -> Result<Frame, VfsError> {
    let entries: Vec<FileEntry> = lister.list_boxed(&path, cancel.clone()).try_collect::<Vec<_>>().await?.concat();
    let pending = entries.iter().filter(|e| e.kind() == EntryKind::Directory).map(|e| e.path().clone()).collect();
    Ok(Frame { path, entries, pending, measured: Vec::new() })
}

fn files_in(frame: &Frame) -> u64 {
    frame.entries.iter().filter(|e| e.kind() != EntryKind::Directory).count() as u64
}

opaque_debug!(MeasureDiskUsage);
