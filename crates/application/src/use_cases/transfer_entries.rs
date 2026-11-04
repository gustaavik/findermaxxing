use std::sync::Arc;

use domain::{CostPolicy, OperationKind, StoragePath};

use crate::operations::transfer::{self, TransferMode};
use crate::operations::{JobContext, OperationHistory, OperationReport};
use crate::ports::MountResolver;

/// Items to put into one destination folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRequest {
    pub sources: Vec<StoragePath>,
    pub destination: StoragePath,
}

/// Copies items into a folder, asking about name clashes. Undo trashes the copies.
pub struct CopyEntries {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    costs: CostPolicy,
}

impl CopyEntries {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history, costs: CostPolicy::default() }
    }

    #[must_use]
    pub fn with_cost_policy(self, costs: CostPolicy) -> Self {
        Self { costs, ..self }
    }

    pub async fn execute(&self, request: &TransferRequest, ctx: &JobContext) -> OperationReport {
        let report =
            transfer::run(&*self.mounts, TransferMode::Copy, &request.sources, &request.destination, self.costs, ctx)
                .await;
        self.history.record(OperationKind::Copy, report.done.clone());
        report
    }
}

/// Moves items into a folder (rename when possible, else copy + delete). Undo moves them back.
pub struct MoveEntries {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    costs: CostPolicy,
}

impl MoveEntries {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history, costs: CostPolicy::default() }
    }

    #[must_use]
    pub fn with_cost_policy(self, costs: CostPolicy) -> Self {
        Self { costs, ..self }
    }

    pub async fn execute(&self, request: &TransferRequest, ctx: &JobContext) -> OperationReport {
        let report =
            transfer::run(&*self.mounts, TransferMode::Move, &request.sources, &request.destination, self.costs, ctx)
                .await;
        self.history.record(OperationKind::Move, report.done.clone());
        report
    }
}

impl std::fmt::Debug for CopyEntries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopyEntries").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for MoveEntries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoveEntries").finish_non_exhaustive()
    }
}
