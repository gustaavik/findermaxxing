use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use domain::{ChangeEvent, StoragePath, VfsError};
use futures::stream::{self, BoxStream};
use futures::{FutureExt, StreamExt};

use crate::ports::{Clock, MountResolver};
use crate::use_cases::HiddenFiles;

/// Coalesced changes: at most one event per path, or a single `Rescan`.
pub type ChangeBatch = Vec<ChangeEvent>;

/// How long to wait after the first event of a burst before emitting, so a
/// `git checkout` touching 10k files becomes one UI update instead of 10k.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoalesceWindow(pub Duration);

impl Default for CoalesceWindow {
    fn default() -> Self {
        const DEFAULT_WINDOW: Duration = Duration::from_millis(100);
        Self(DEFAULT_WINDOW)
    }
}

/// Watches a directory and emits coalesced, hidden-filtered change batches.
pub struct WatchDirectory {
    mounts: Arc<dyn MountResolver>,
    clock: Arc<dyn Clock>,
    window: CoalesceWindow,
}

impl WatchDirectory {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, clock: Arc<dyn Clock>, window: CoalesceWindow) -> Self {
        Self { mounts, clock, window }
    }

    /// # Errors
    /// Propagates provider errors (`Unsupported(Watch)`, `NotFound`, …).
    pub fn execute(&self, dir: &StoragePath, hidden: HiddenFiles) -> Result<BoxStream<'static, ChangeBatch>, VfsError> {
        let events = self.mounts.resolve(dir.mount())?.watcher()?.watch_boxed(dir)?;
        let clock = Arc::clone(&self.clock);
        let window = self.window.0;
        let batches = stream::unfold(events, move |mut events| {
            let clock = Arc::clone(&clock);
            async move {
                let first = events.next().await?;
                clock.sleep(window).await;
                let mut burst = vec![first];
                while let Some(Some(event)) = events.next().now_or_never() {
                    burst.push(event);
                }
                Some((coalesce(burst, hidden), events))
            }
        });
        Ok(batches.filter(|batch| futures::future::ready(!batch.is_empty())).boxed())
    }
}

/// Keeps the last event per path in first-seen order; any `Rescan` wins.
fn coalesce(events: Vec<ChangeEvent>, hidden: HiddenFiles) -> ChangeBatch {
    let mut order: Vec<StoragePath> = Vec::new();
    let mut latest: HashMap<StoragePath, ChangeEvent> = HashMap::new();
    for event in events {
        let Some(path) = event.path().cloned() else {
            return vec![ChangeEvent::Rescan];
        };
        if latest.insert(path.clone(), event).is_none() {
            order.push(path);
        }
    }
    order.into_iter().filter_map(|path| latest.remove(&path)).filter(|event| admits(event, hidden)).collect()
}

fn admits(event: &ChangeEvent, hidden: HiddenFiles) -> bool {
    match event {
        ChangeEvent::Upserted(entry) => hidden.admits(entry.is_hidden()),
        ChangeEvent::Removed(_) | ChangeEvent::Rescan => true,
    }
}

impl std::fmt::Debug for WatchDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchDirectory").field("window", &self.window).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderRegistry;
    use crate::testing::{FakeVfs, InstantClock};
    use futures_executor::block_on;

    fn use_case(fake: &FakeVfs) -> WatchDirectory {
        let registry = ProviderRegistry::default().with_provider(fake.clone().into_factory()).unwrap();
        WatchDirectory::new(Arc::new(registry), Arc::new(InstantClock), CoalesceWindow::default())
    }

    #[test]
    fn burst_is_coalesced_to_last_event_per_path() {
        let fake = FakeVfs::new();
        fake.add_dir("/w");
        let mut batches = use_case(&fake).execute(&fake.path("/w"), HiddenFiles::Hide).unwrap();
        fake.add_file("/w/a", 1);
        fake.add_file("/w/b", 1);
        fake.remove("/w/a");
        fake.add_file("/w/.hidden", 1);

        let batch = block_on(batches.next()).unwrap();
        assert_eq!(batch, vec![ChangeEvent::Removed(fake.path("/w/a")), fake.upserted("/w/b")]);
    }

    #[test]
    fn rescan_replaces_whole_burst() {
        let fake = FakeVfs::new();
        fake.add_dir("/w");
        let mut batches = use_case(&fake).execute(&fake.path("/w"), HiddenFiles::Hide).unwrap();
        fake.add_file("/w/a", 1);
        fake.emit_rescan();
        assert_eq!(block_on(batches.next()).unwrap(), vec![ChangeEvent::Rescan]);
    }

    #[test]
    fn watching_missing_dir_fails() {
        let fake = FakeVfs::new();
        let result = use_case(&fake).execute(&fake.path("/missing"), HiddenFiles::Hide);
        assert!(matches!(result, Err(VfsError::NotFound)));
    }
}
