use std::fmt;
use std::sync::Arc;

use domain::{Conflict, ConflictChoice, ConflictResolution, ConflictScope, CostDecision, CostWarning};
use futures::FutureExt;
use futures::future::{self, BoxFuture};

use crate::cancel::CancelToken;
use crate::ports::{ConflictResolver, CostConfirmer, JobObserver};

/// Everything a running job needs from its owner.
#[derive(Clone)]
pub struct JobContext {
    pub observer: Arc<dyn JobObserver>,
    pub resolver: Arc<dyn ConflictResolver>,
    pub costs: Arc<dyn CostConfirmer>,
    pub cancel: CancelToken,
}

impl JobContext {
    /// For jobs that must never prompt (undo/redo): any conflict stops the job,
    /// and costs are accepted (undo only deletes or moves back what a job did).
    #[must_use]
    pub fn unattended(observer: Arc<dyn JobObserver>, cancel: CancelToken) -> Self {
        Self { observer, resolver: Arc::new(StopOnConflict), costs: Arc::new(AcceptCosts), cancel }
    }
}

impl fmt::Debug for JobContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobContext").field("cancelled", &self.cancel.is_cancelled()).finish_non_exhaustive()
    }
}

/// For jobs whose progress nobody watches (e.g. a quick bulk rename).
#[derive(Debug, Clone, Copy, Default)]
pub struct NullObserver;

impl JobObserver for NullObserver {
    fn progress(&self, _progress: &domain::JobProgress) {}
}

#[derive(Debug)]
struct StopOnConflict;

impl ConflictResolver for StopOnConflict {
    fn resolve(&self, _conflict: Conflict) -> BoxFuture<'static, ConflictResolution> {
        future::ready(ConflictResolution { choice: ConflictChoice::Stop, scope: ConflictScope::AllRemaining }).boxed()
    }
}

/// Never asks; used where the user already agreed or no new cost arises.
#[derive(Debug, Clone, Copy, Default)]
pub struct AcceptCosts;

impl CostConfirmer for AcceptCosts {
    fn confirm(&self, _warning: CostWarning) -> BoxFuture<'static, CostDecision> {
        future::ready(CostDecision::Proceed).boxed()
    }
}
