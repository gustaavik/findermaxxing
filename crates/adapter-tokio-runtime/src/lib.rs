//! Implements [`application::ports::Clock`] on tokio's timer.

use std::time::Duration;

use application::ports::Clock;
use futures::FutureExt;
use futures::future::BoxFuture;

#[derive(Debug, Clone, Copy, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        tokio::time::sleep(duration).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn sleeps_on_the_tokio_timer() {
        let start = tokio::time::Instant::now();
        TokioClock.sleep(Duration::from_secs(3)).await;
        assert_eq!(start.elapsed().as_secs(), 3);
    }
}
