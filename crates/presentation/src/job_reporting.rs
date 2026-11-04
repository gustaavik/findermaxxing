//! Port implementations that carry job events from worker threads to the
//! Qt thread: throttled progress, and conflict questions awaiting an answer.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use application::ports::{ConflictResolver, JobObserver};
use cxx_qt::CxxQtThread;
use domain::{Conflict, ConflictChoice, ConflictResolution, ConflictScope, JobProgress};
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
