//! Applies the accepted suggestions as one undoable step: new folders, then
//! the Trash, then moves and renames, then tags (ADR 0029).

use std::sync::Arc;

use domain::{Action, EntryKind, Operation, OperationKind, Resolution, StoragePath, Tag, VfsError, merged_tags};

use crate::operations::executor::{self, NoTrash, set_tags_all};
use crate::operations::progress::ProgressTracker;
use crate::operations::{JobContext, OperationError, OperationHistory, OperationReport};
use crate::ports::MountResolver;
use crate::use_cases::utilities::{free_path, opaque_debug};

pub struct ApplySuggestions {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl ApplySuggestions {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history }
    }

    /// Everything done is recorded (also when it stops part-way), so Undo
    /// reverts exactly that.
    pub async fn execute(&self, resolution: &Resolution, ctx: &JobContext) -> OperationReport {
        let mut done = Vec::new();
        let result = self.run(resolution, ctx, &mut done).await;
        let done = Operation::sequence(done);
        self.history.record(OperationKind::Organize, done.clone());
        OperationReport::new(done, result)
    }

    async fn run(
        &self,
        resolution: &Resolution,
        ctx: &JobContext,
        done: &mut Vec<Operation>,
    ) -> Result<(), OperationError> {
        for folder in &resolution.folders_to_create {
            let created = async {
                let mount = self.mounts.resolve(folder.mount())?;
                mount.folder_maker()?.create_folder_boxed(folder).await
            };
            match created.await {
                Ok(_) => done.push(Operation::CreatedFolder(folder.clone())),
                // Made in the meantime: use it.
                Err(VfsError::AlreadyExists) => {}
                Err(error) => return Err(OperationError::on(folder)(error)),
            }
        }
        let moves = self.free_targets(resolution).await?;
        let steps = Action::Sequence(vec![Action::Trash(resolution.trash.clone()), Action::Move(moves)]);
        let report = executor::execute(&*self.mounts, &steps, NoTrash::Refuse, ctx).await;
        done.push(report.done);
        report.result?;
        let tags = self.merged(&resolution.tags).await?;
        let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
        let (tagged, result) = set_tags_all(&*self.mounts, &tags, &tracker, ctx).await;
        done.push(tagged);
        result
    }

    /// Targets inside existing subfolders weren't known when the plan was
    /// resolved: anything taken there now gets a numbered name.
    async fn free_targets(&self, resolution: &Resolution) -> Result<Vec<domain::Relocation>, OperationError> {
        let mut moves = Vec::with_capacity(resolution.moves.len());
        for relocation in &resolution.moves {
            let target = &relocation.to;
            let (Some(parent), Some(name)) = (target.parent(), target.name()) else { continue };
            let free = async {
                let mount = self.mounts.resolve(target.mount())?;
                let kind = mount.lister()?.stat_boxed(&relocation.from).await?.kind();
                if relocation.from.parent().as_ref() == Some(&parent) {
                    // A rename in place: the plan already avoided the folder's names.
                    return Ok(target.clone());
                }
                free_path(&mount, &parent, name, if kind == EntryKind::Directory { kind } else { EntryKind::File })
                    .await
            };
            let to = free.await.map_err(OperationError::on(&relocation.from))?;
            moves.push(domain::Relocation { from: relocation.from.clone(), to });
        }
        Ok(moves)
    }

    /// The tags each item ends up with: what it had plus what was accepted.
    async fn merged(&self, added: &[(StoragePath, Vec<Tag>)]) -> Result<Vec<(StoragePath, Vec<Tag>)>, OperationError> {
        let mut merged = Vec::with_capacity(added.len());
        for (path, tags) in added {
            let existing = async {
                let editor = self.mounts.resolve(path.mount())?.tag_editor()?;
                editor.tags(path).await
            };
            let existing = existing.await.map_err(OperationError::on(path))?;
            merged.push((path.clone(), merged_tags(&existing, tags)));
        }
        Ok(merged)
    }
}

opaque_debug!(ApplySuggestions);
