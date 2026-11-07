//! Reading and changing POSIX permissions of a selection, optionally of
//! everything inside it too. Changes are undoable (each item's previous
//! mode is recorded).

use std::sync::Arc;

use domain::{
    ByteSize, Capability, EntryKind, FileEntry, Operation, OperationKind, PermissionEdit, PermissionReach, StoragePath,
    UnixMode, VfsError,
};

use super::checksums::opaque_debug;
use crate::operations::executor::set_permissions_all;
use crate::operations::progress::ProgressTracker;
use crate::operations::scan::scan;
use crate::operations::{JobContext, OperationError, OperationHistory, OperationReport};
use crate::ports::MountResolver;

/// What the dialog shows for a selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionsView {
    /// One per item with known permissions.
    pub modes: Vec<UnixMode>,
    pub editable: bool,
    pub has_folders: bool,
}

pub struct GetPermissions {
    mounts: Arc<dyn MountResolver>,
}

impl GetPermissions {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// # Errors
    /// An item couldn't be looked up.
    pub async fn execute(&self, items: &[StoragePath]) -> Result<PermissionsView, VfsError> {
        let mut view = PermissionsView { modes: Vec::new(), editable: !items.is_empty(), has_folders: false };
        for item in items {
            let mount = self.mounts.resolve(item.mount())?;
            view.editable &= mount.capabilities().contains(Capability::EditPermissions);
            let entry = mount.lister()?.stat_boxed(item).await?;
            view.has_folders |= entry.kind() == EntryKind::Directory;
            view.modes.extend(entry.metadata().permissions);
        }
        Ok(view)
    }
}

pub struct ChangePermissions {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl ChangePermissions {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history }
    }

    /// Applies `edit` to every item `reach` covers; items already as wanted
    /// (and links, whose permissions mean nothing) are left alone.
    pub async fn execute(
        &self,
        items: &[StoragePath],
        edit: PermissionEdit,
        reach: PermissionReach,
        ctx: &JobContext,
    ) -> OperationReport {
        let targets = match self.targets(items, reach, ctx).await {
            Ok(targets) => targets,
            Err(error) => return OperationReport::new(Operation::sequence(Vec::new()), Err(error)),
        };
        let modes: Vec<(StoragePath, UnixMode)> = targets
            .into_iter()
            .filter_map(|entry| {
                let before = entry.metadata().permissions?;
                let after = edit.apply(before);
                (after != before).then(|| (entry.path().clone(), after))
            })
            .collect();
        let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
        tracker.begin(modes.len() as u64, ByteSize(0));
        let (done, result) = set_permissions_all(&*self.mounts, &modes, &tracker, ctx).await;
        self.history.record(OperationKind::ChangePermissions, done.clone());
        OperationReport::new(done, result)
    }

    async fn targets(
        &self,
        items: &[StoragePath],
        reach: PermissionReach,
        ctx: &JobContext,
    ) -> Result<Vec<FileEntry>, OperationError> {
        let mut targets = Vec::new();
        for item in items {
            let found = async {
                let lister = self.mounts.resolve(item.mount())?.lister()?;
                let root = lister.stat_boxed(item).await?;
                if reach == PermissionReach::SelectedItems || root.kind() != EntryKind::Directory {
                    return Ok(vec![root]);
                }
                let tree = scan(&*lister, root, &ctx.cancel).await?;
                let (first, rest) = tree.items.split_at(1);
                let inside = rest.iter().filter(|e| match reach {
                    PermissionReach::AlsoFilesInside => e.kind() == EntryKind::File,
                    PermissionReach::AlsoFoldersInside => e.kind() == EntryKind::Directory,
                    _ => matches!(e.kind(), EntryKind::File | EntryKind::Directory),
                });
                Ok::<_, VfsError>(first.iter().chain(inside).cloned().collect::<Vec<_>>())
            };
            targets.extend(found.await.map_err(OperationError::on(item))?);
        }
        Ok(targets)
    }
}

opaque_debug!(GetPermissions, ChangePermissions);
