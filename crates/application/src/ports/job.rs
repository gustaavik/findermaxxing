use domain::{Conflict, ConflictResolution, JobProgress};
use futures::future::BoxFuture;

/// Receives progress of one running job. Called often; implementations
/// throttle as they see fit (the UI does).
pub trait JobObserver: Send + Sync {
    fn progress(&self, progress: &JobProgress);
}

/// Asks the user what to do about a name clash. The job waits for the answer.
pub trait ConflictResolver: Send + Sync {
    fn resolve(&self, conflict: Conflict) -> BoxFuture<'static, ConflictResolution>;
}
