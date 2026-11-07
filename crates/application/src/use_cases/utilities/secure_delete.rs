//! Overwrites files before deleting them, on providers that can (local
//! disks). Irreversible by design, so nothing is recorded for undo.
//!
//! On SSDs and copy-on-write file systems (APFS, Btrfs) the old blocks may
//! survive an overwrite; the UI says so (ADR 0027). Full-disk encryption is
//! what really protects deleted data.

use std::sync::Arc;

use domain::{ByteSize, EntryKind, StoragePath};

use super::checksums::opaque_debug;
use crate::operations::progress::ProgressTracker;
use crate::operations::scan::{ScannedTree, scan};
use crate::operations::{JobContext, OperationError};
use crate::ports::MountResolver;

/// One pass of random data: more passes add nothing on modern drives.
pub const SECURE_DELETE_PASSES: u8 = 1;

pub struct SecureDelete {
    mounts: Arc<dyn MountResolver>,
}

impl SecureDelete {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// Overwrites and removes every file below the items, then the folders.
    ///
    /// # Errors
    /// The first failure; what was erased before it stays erased.
    pub async fn execute(&self, items: &[StoragePath], ctx: &JobContext) -> Result<(), OperationError> {
        let mut trees = Vec::with_capacity(items.len());
        for item in items {
            let tree = async {
                let mount = self.mounts.resolve(item.mount())?;
                // Refuse up front rather than after erasing half a selection.
                mount.secure_eraser()?;
                let lister = mount.lister()?;
                let root = lister.stat_boxed(item).await?;
                scan(&*lister, root, &ctx.cancel).await
            };
            trees.push(tree.await.map_err(OperationError::on(item))?);
        }
        let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
        tracker.begin(
            trees.iter().map(ScannedTree::item_count).sum(),
            ByteSize(trees.iter().map(|t| t.bytes.0).sum::<u64>() * u64::from(SECURE_DELETE_PASSES)),
        );
        for tree in &trees {
            self.erase_tree(tree, &tracker, ctx).await?;
        }
        Ok(())
    }

    async fn erase_tree(
        &self,
        tree: &ScannedTree,
        tracker: &Arc<ProgressTracker>,
        ctx: &JobContext,
    ) -> Result<(), OperationError> {
        // Pre-order reversed: contents before their folder.
        for item in tree.items.iter().rev() {
            if ctx.cancel.is_cancelled() {
                return Err(OperationError::Cancelled);
            }
            let path = item.path();
            tracker.item_started(path.name());
            let mount = self.mounts.resolve(path.mount()).map_err(OperationError::on(path))?;
            let result = match item.kind() {
                EntryKind::File => match mount.secure_eraser() {
                    Ok(eraser) => {
                        eraser.erase(path, SECURE_DELETE_PASSES, tracker.byte_callback(), ctx.cancel.clone()).await
                    }
                    Err(error) => Err(error),
                },
                // Folders are empty by now; links and others hold no data of their own.
                _ => match mount.remover() {
                    Ok(remover) => remover.remove_boxed(path, ctx.cancel.clone()).await,
                    Err(error) => Err(error),
                },
            };
            result.map_err(OperationError::on(path))?;
            tracker.item_finished();
        }
        Ok(())
    }
}

opaque_debug!(SecureDelete);
