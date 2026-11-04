//! Port implementations that carry job events from worker threads to the
//! Qt thread: throttled progress, and conflict and cost questions awaiting
//! an answer.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use application::ports::{ConflictResolver, CostConfirmer, JobObserver};
use application::use_cases::ListS3Accounts;
use cxx_qt::CxxQtThread;
use domain::{
    AccountId, Conflict, ConflictChoice, ConflictResolution, ConflictScope, CostDecision, CostWarning, JobProgress,
};
use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::BoxFuture;

use crate::job_text::progress_detail;
use crate::operations_controller::qobject::OperationsController;

/// Progress reaches the UI at most this often (plus the final 100 % update).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct QtJobObserver {
    job_id: i32,
    thread: CxxQtThread<OperationsController>,
    last_sent: Mutex<Option<Instant>>,
}

impl QtJobObserver {
    pub(crate) fn new(job_id: i32, thread: CxxQtThread<OperationsController>) -> Self {
        Self { job_id, thread, last_sent: Mutex::new(None) }
    }

    fn is_due(&self, progress: &JobProgress) -> bool {
        let mut last = self.last_sent.lock().unwrap_or_else(PoisonError::into_inner);
        let complete = progress.fraction() == Some(1.0);
        let due = complete || last.is_none_or(|at| at.elapsed() >= PROGRESS_INTERVAL);
        if due {
            *last = Some(Instant::now());
        }
        due
    }
}

impl JobObserver for QtJobObserver {
    fn progress(&self, progress: &JobProgress) {
        if !self.is_due(progress) {
            return;
        }
        let (detail, fraction) = progress_detail(progress);
        let job_id = self.job_id;
        // Fails only if the window is gone.
        let _ = self.thread.queue(move |controller| controller.update_job(job_id, detail, fraction));
    }
}

pub(crate) struct QtConflictResolver {
    job_id: i32,
    thread: CxxQtThread<OperationsController>,
}

impl QtConflictResolver {
    pub(crate) fn new(job_id: i32, thread: CxxQtThread<OperationsController>) -> Self {
        Self { job_id, thread }
    }
}

impl ConflictResolver for QtConflictResolver {
    fn resolve(&self, conflict: Conflict) -> BoxFuture<'static, ConflictResolution> {
        let (reply, answer) = oneshot::channel();
        let job_id = self.job_id;
        let _ = self.thread.queue(move |controller| controller.enqueue_conflict(job_id, conflict, reply));
        // An unanswerable question (window closed, job cancelled) stops the job.
        let stop = ConflictResolution { choice: ConflictChoice::Stop, scope: ConflictScope::AllRemaining };
        answer.map(move |answer| answer.unwrap_or(stop)).boxed()
    }
}

/// Asks about costs unless the user opted out for every account involved.
pub(crate) struct QtCostConfirmer {
    job_id: i32,
    thread: CxxQtThread<OperationsController>,
    accounts: Vec<AccountId>,
    list_accounts: Arc<ListS3Accounts>,
}

impl QtCostConfirmer {
    pub(crate) fn new(
        job_id: i32,
        thread: CxxQtThread<OperationsController>,
        accounts: Vec<AccountId>,
        list_accounts: Arc<ListS3Accounts>,
    ) -> Self {
        Self { job_id, thread, accounts, list_accounts }
    }
}

impl CostConfirmer for QtCostConfirmer {
    fn confirm(&self, warning: CostWarning) -> BoxFuture<'static, CostDecision> {
        let (job_id, thread, accounts, list) =
            (self.job_id, self.thread.clone(), self.accounts.clone(), Arc::clone(&self.list_accounts));
        async move {
            let stored = list.execute().await.unwrap_or_default();
            let all_opted_out = !accounts.is_empty()
                && accounts.iter().all(|id| stored.iter().any(|a| a.id == *id && a.skip_cost_warnings));
            if all_opted_out {
                return CostDecision::Proceed;
            }
            let (reply, answer) = oneshot::channel();
            let _ = thread.queue(move |controller| controller.enqueue_cost(job_id, warning, accounts, reply));
            // An unanswerable question (window closed, job cancelled) cancels the transfer.
            answer.await.unwrap_or(CostDecision::Cancel)
        }
        .boxed()
    }
}
