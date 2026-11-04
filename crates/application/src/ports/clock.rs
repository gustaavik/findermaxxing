use std::time::Duration;

use futures::future::BoxFuture;

/// Time source, injected so that debouncing and throttling are testable
/// without a real runtime.
pub trait Clock: Send + Sync {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}
