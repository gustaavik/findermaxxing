use std::sync::{Arc, Mutex, PoisonError};

use domain::{ByteSize, JobPhase, JobProgress, PathSegment};

use crate::ports::{JobObserver, ProgressCallback};

/// Accumulates progress and forwards every change to the observer. Shared
/// with copier callbacks, which may run on other threads.
pub(crate) struct ProgressTracker {
    observer: Arc<dyn JobObserver>,
    state: Mutex<JobProgress>,
}

impl ProgressTracker {
    pub(crate) fn new(observer: Arc<dyn JobObserver>) -> Arc<Self> {
        let tracker = Arc::new(Self { observer, state: Mutex::new(JobProgress::preparing()) });
        tracker.update(|_| {});
        tracker
    }

    pub(crate) fn begin(&self, items: u64, bytes: ByteSize) {
        self.update(|p| {
            p.phase = JobPhase::Transferring;
            p.items_total = items;
            p.bytes_total = bytes;
        });
    }

    /// For work discovered late (a move that turned into copy + delete).
    pub(crate) fn add_totals(&self, items: u64, bytes: ByteSize) {
        self.update(|p| {
            p.items_total += items;
            p.bytes_total = ByteSize(p.bytes_total.0 + bytes.0);
        });
    }

    pub(crate) fn item_started(&self, name: Option<&PathSegment>) {
        self.update(|p| p.current_item = name.cloned());
    }

    pub(crate) fn item_finished(&self) {
        self.update(|p| p.items_done += 1);
    }

    /// Skipped items count as done so the bar still reaches 100 %.
    pub(crate) fn skipped(&self, items: u64, bytes: ByteSize) {
        self.update(|p| {
            p.items_done += items;
            p.bytes_done = ByteSize(p.bytes_done.0 + bytes.0);
        });
    }

    pub(crate) fn byte_callback(self: &Arc<Self>) -> ProgressCallback {
        let tracker = Arc::clone(self);
        Arc::new(move |bytes| tracker.update(|p| p.bytes_done = ByteSize(p.bytes_done.0 + bytes.0)))
    }

    fn update(&self, change: impl FnOnce(&mut JobProgress)) {
        let snapshot = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            change(&mut state);
            state.clone()
        };
        self.observer.progress(&snapshot);
    }
}
