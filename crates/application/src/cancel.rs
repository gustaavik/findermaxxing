use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{self, BoxFuture};

/// Owner side of a cancellation scope. Dropping the source does *not* cancel;
/// cancellation is always an explicit decision by the task owner.
#[derive(Debug, Default)]
pub struct CancellationSource {
    shared: Arc<Shared>,
}

/// Cheap, cloneable observer handed to every long-running operation.
#[derive(Debug, Clone)]
pub struct CancelToken {
    shared: Arc<Shared>,
}

#[derive(Debug, Default)]
struct Shared {
    cancelled: AtomicBool,
    // Shared mutable state is intentional: waiters register from any thread
    // and are woken once by the owner.
    waiters: Mutex<Vec<oneshot::Sender<()>>>,
}

impl CancellationSource {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn token(&self) -> CancelToken {
        CancelToken { shared: Arc::clone(&self.shared) }
    }

    pub fn cancel(&self) {
        let mut waiters = self.shared.waiters.lock().unwrap_or_else(PoisonError::into_inner);
        self.shared.cancelled.store(true, Ordering::Release);
        for waiter in waiters.drain(..) {
            // A dropped receiver just means nobody is waiting any more.
            let _ = waiter.send(());
        }
    }
}

impl CancelToken {
    /// A token that is never cancelled, for short operations and tests.
    #[must_use]
    pub fn never() -> Self {
        CancellationSource::new().token()
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Acquire)
    }

    /// Resolves once the owning [`CancellationSource`] cancels.
    pub fn cancelled(&self) -> BoxFuture<'static, ()> {
        let mut waiters = self.shared.waiters.lock().unwrap_or_else(PoisonError::into_inner);
        if self.is_cancelled() {
            return future::ready(()).boxed();
        }
        let (sender, receiver) = oneshot::channel();
        waiters.push(sender);
        receiver.map(|_| ()).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;

    #[test]
    fn cancel_is_observed_by_all_clones() {
        let source = CancellationSource::new();
        let token = source.token();
        let clone = token.clone();
        let waiting = token.cancelled();
        assert!(!clone.is_cancelled());
        source.cancel();
        assert!(clone.is_cancelled());
        block_on(waiting);
        block_on(token.cancelled());
    }
}
