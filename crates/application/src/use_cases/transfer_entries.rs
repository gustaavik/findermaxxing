use std::sync::Arc;

use domain::{CostPolicy, OperationKind, StoragePath};

use crate::operations::resume::Resumption;
use crate::operations::transfer::{self, TransferMode};
use crate::operations::{JobContext, OperationHistory, OperationReport};
use crate::ports::{InterruptedTransfer, JournalEntry, JournaledKind, MountResolver, StoreError, TransferJournal};

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
    journal: Option<Arc<dyn TransferJournal>>,
}

impl CopyEntries {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history, costs: CostPolicy::default(), journal: None }
    }

    #[must_use]
    pub fn with_cost_policy(self, costs: CostPolicy) -> Self {
        Self { costs, ..self }
    }

    /// Records the job so it can be resumed if the app stops mid-way.
    #[must_use]
    pub fn with_journal(self, journal: Arc<dyn TransferJournal>) -> Self {
        Self { journal: Some(journal), ..self }
    }

    pub async fn execute(&self, request: &TransferRequest, ctx: &JobContext) -> OperationReport {
        let report =
            journaled(&*self.mounts, self.journal.as_ref(), TransferMode::Copy, request, self.costs, ctx).await;
        self.history.record(OperationKind::Copy, report.done.clone());
        report
    }
}

/// Moves items into a folder (rename when possible, else copy + delete). Undo moves them back.
pub struct MoveEntries {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    costs: CostPolicy,
    journal: Option<Arc<dyn TransferJournal>>,
}

impl MoveEntries {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, history: Arc<OperationHistory>) -> Self {
        Self { mounts, history, costs: CostPolicy::default(), journal: None }
    }

    #[must_use]
    pub fn with_cost_policy(self, costs: CostPolicy) -> Self {
        Self { costs, ..self }
    }

    /// Records the job so it can be resumed if the app stops mid-way.
    #[must_use]
    pub fn with_journal(self, journal: Arc<dyn TransferJournal>) -> Self {
        Self { journal: Some(journal), ..self }
    }

    pub async fn execute(&self, request: &TransferRequest, ctx: &JobContext) -> OperationReport {
        let report =
            journaled(&*self.mounts, self.journal.as_ref(), TransferMode::Move, request, self.costs, ctx).await;
        self.history.record(OperationKind::Move, report.done.clone());
        report
    }
}

/// Runs a fresh transfer, journaled when a journal is configured. If the
/// future is dropped (the app quits), the journal entry stays behind.
async fn journaled(
    mounts: &dyn MountResolver,
    journal: Option<&Arc<dyn TransferJournal>>,
    mode: TransferMode,
    request: &TransferRequest,
    costs: CostPolicy,
    ctx: &JobContext,
) -> OperationReport {
    let entry = JournalEntry {
        kind: journal_kind(mode),
        sources: request.sources.clone(),
        destination: request.destination.clone(),
    };
    let id = match journal {
        Some(journal) => journal.begin(entry).await.ok().map(|id| (Arc::clone(journal), id)),
        None => None,
    };
    let resumption = match &id {
        Some((journal, id)) => Resumption::journaled(Arc::clone(journal), *id),
        None => Resumption::none(),
    };
    let report = transfer::run(mounts, mode, &request.sources, &request.destination, costs, ctx, &resumption).await;
    if let Some((journal, id)) = id {
        let _ = journal.finish(id).await;
    }
    report
}

fn journal_kind(mode: TransferMode) -> JournaledKind {
    match mode {
        TransferMode::Copy => JournaledKind::Copy,
        TransferMode::Move => JournaledKind::Move,
    }
}

/// Jobs a crash or quit cut short, oldest first.
pub struct ListInterruptedTransfers {
    journal: Arc<dyn TransferJournal>,
}

impl ListInterruptedTransfers {
    #[must_use]
    pub fn new(journal: Arc<dyn TransferJournal>) -> Self {
        Self { journal }
    }

    /// # Errors
    /// The journal can't be read.
    pub async fn execute(&self) -> Result<Vec<InterruptedTransfer>, StoreError> {
        self.journal.interrupted().await
    }
}

/// Runs an interrupted job again. What the earlier run finished is kept (and
/// not asked about); a large file it was writing continues where it stopped.
pub struct ResumeTransfer {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    journal: Arc<dyn TransferJournal>,
}

impl ResumeTransfer {
    #[must_use]
    pub fn new(
        mounts: Arc<dyn MountResolver>,
        history: Arc<OperationHistory>,
        journal: Arc<dyn TransferJournal>,
    ) -> Self {
        Self { mounts, history, journal }
    }

    /// Cost warnings were answered by the first run, so they are not repeated.
    pub async fn execute(&self, interrupted: &InterruptedTransfer, ctx: &JobContext) -> OperationReport {
        let (mode, kind) = match interrupted.entry.kind {
            JournaledKind::Copy => (TransferMode::Copy, OperationKind::Copy),
            JournaledKind::Move => (TransferMode::Move, OperationKind::Move),
        };
        let resumption =
            Resumption::continuing(Arc::clone(&self.journal), interrupted.id, interrupted.in_flight.clone());
        let ctx = JobContext { costs: Arc::new(crate::operations::AcceptCosts), ..ctx.clone() };
        let entry = &interrupted.entry;
        let report = transfer::run(
            &*self.mounts,
            mode,
            &entry.sources,
            &entry.destination,
            CostPolicy::default(),
            &ctx,
            &resumption,
        )
        .await;
        if let Some(stale) = resumption.take_stale() {
            discard_write(&*self.mounts, &stale).await;
        }
        let _ = self.journal.finish(interrupted.id).await;
        self.history.record(kind, report.done.clone());
        report
    }
}

/// Forgets an interrupted job and deletes its partial write.
pub struct DiscardTransfer {
    mounts: Arc<dyn MountResolver>,
    journal: Arc<dyn TransferJournal>,
}

impl DiscardTransfer {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, journal: Arc<dyn TransferJournal>) -> Self {
        Self { mounts, journal }
    }

    /// # Errors
    /// The journal can't be updated. Leftovers that can't be deleted are
    /// ignored: the provider (or the user) cleans them up.
    pub async fn execute(&self, interrupted: &InterruptedTransfer) -> Result<(), StoreError> {
        if let Some(write) = &interrupted.in_flight {
            discard_write(&*self.mounts, write).await;
        }
        self.journal.finish(interrupted.id).await
    }
}

async fn discard_write(mounts: &dyn MountResolver, write: &crate::ports::InFlightWrite) {
    if let Ok(writer) = mounts.resolve(write.target.mount()).and_then(|handle| handle.writer()) {
        let _ = writer.discard_boxed(&write.target, &write.state).await;
    }
}

impl std::fmt::Debug for ListInterruptedTransfers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListInterruptedTransfers").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ResumeTransfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeTransfer").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for DiscardTransfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscardTransfer").finish_non_exhaustive()
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
