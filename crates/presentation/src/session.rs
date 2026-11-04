//! One open folder: lists it, keeps it sorted, applies watcher changes, and
//! reports model updates. A session is a single task that owns all of its
//! state; the controller only sends commands and receives events.

use std::time::Duration;

use application::use_cases::{ChangeBatch, HiddenFiles, ListRequest};
use application::{CancelToken, CancellationSource};
use domain::{SortSpec, StoragePath, VfsError};
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::services::AppServices;
use crate::view_model::{ChangeOutcome, DirectoryViewModel, ModelUpdate};

/// While a large folder is still loading, re-publish the sorted listing at
/// most this often so the view fills progressively without thrashing.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub(crate) struct SessionConfig {
    pub dir: StoragePath,
    pub hidden: HiddenFiles,
    pub sort: SortSpec,
}

#[derive(Debug)]
pub(crate) enum SessionEvent {
    Model(Vec<ModelUpdate>),
    Loading(bool),
    Failed(VfsError),
}

#[derive(Debug)]
enum SessionCommand {
    Sort(SortSpec),
    Reload,
}

/// Receives events on the session task's thread. The controller's sink
/// forwards them to the Qt thread.
pub(crate) type EventSink = Box<dyn Fn(SessionEvent) + Send + Sync>;

/// Handle owned by the controller. Dropping it cancels the listing and stops
/// the task, so navigating away never leaves work running.
#[derive(Debug)]
pub(crate) struct FolderSession {
    cancellation: CancellationSource,
    commands: mpsc::UnboundedSender<SessionCommand>,
    task: JoinHandle<()>,
}

impl FolderSession {
    pub(crate) fn start(services: &AppServices, config: SessionConfig, sink: EventSink) -> Self {
        let cancellation = CancellationSource::new();
        let (commands, receiver) = mpsc::unbounded_channel();
        let worker = Worker {
            services: services.clone(),
            view_model: DirectoryViewModel::new(config.sort),
            config,
            cancel: cancellation.token(),
            sink,
        };
        let task = services.runtime.spawn(worker.run(receiver));
        Self { cancellation, commands, task }
    }

    pub(crate) fn sort(&self, sort: SortSpec) {
        // A send error means the task already ended; there is nothing to sort.
        let _ = self.commands.send(SessionCommand::Sort(sort));
    }

    pub(crate) fn reload(&self) {
        let _ = self.commands.send(SessionCommand::Reload);
    }
}

impl Drop for FolderSession {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

struct Worker {
    services: AppServices,
    config: SessionConfig,
    view_model: DirectoryViewModel,
    cancel: CancelToken,
    sink: EventSink,
}

impl Worker {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<SessionCommand>) {
        // Subscribe before listing so changes made during a long listing are not lost.
        let mut changes = self.services.watch_directory.execute(&self.config.dir, self.config.hidden).ok();
        if self.load().await.is_err() {
            return;
        }
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => if self.handle(command).await.is_err() { return },
                    None => return,
                },
                batch = next_change(&mut changes) => match batch {
                    Some(batch) => if self.apply(batch).await.is_err() { return },
                    None => changes = None,
                },
            }
        }
    }

    async fn handle(&mut self, command: SessionCommand) -> Result<(), Cancelled> {
        match command {
            SessionCommand::Sort(sort) => {
                (self.sink)(SessionEvent::Model(vec![self.view_model.set_sort(sort)]));
                Ok(())
            }
            SessionCommand::Reload => self.load().await,
        }
    }

    async fn apply(&mut self, batch: ChangeBatch) -> Result<(), Cancelled> {
        match self.view_model.apply(batch) {
            ChangeOutcome::Updates(updates) if updates.is_empty() => Ok(()),
            ChangeOutcome::Updates(updates) => {
                (self.sink)(SessionEvent::Model(updates));
                Ok(())
            }
            ChangeOutcome::RescanRequired => self.load().await,
        }
    }

    /// Streams the listing into the view model, publishing progressively.
    /// Listing errors are reported to the UI and are not fatal to the session.
    async fn load(&mut self) -> Result<(), Cancelled> {
        self.view_model.clear();
        (self.sink)(SessionEvent::Loading(true));
        let request = ListRequest { dir: self.config.dir.clone(), hidden: self.config.hidden };
        let mut batches = self.services.list_directory.execute(&request, self.cancel.clone());
        let mut last_publish: Option<Instant> = None;
        let outcome = loop {
            match batches.next().await {
                Some(Ok(batch)) => {
                    self.view_model.add_batch(batch);
                    if last_publish.is_none_or(|at| at.elapsed() >= PUBLISH_INTERVAL) {
                        (self.sink)(SessionEvent::Model(vec![self.view_model.snapshot()]));
                        last_publish = Some(Instant::now());
                    }
                }
                Some(Err(VfsError::Cancelled)) => return Err(Cancelled),
                Some(Err(error)) => break Some(error),
                None => break None,
            }
        };
        (self.sink)(SessionEvent::Model(vec![self.view_model.snapshot()]));
        if let Some(error) = outcome {
            (self.sink)(SessionEvent::Failed(error));
        }
        (self.sink)(SessionEvent::Loading(false));
        Ok(())
    }
}

/// The session was cancelled; the task should end quietly.
#[derive(Debug)]
struct Cancelled;

async fn next_change(changes: &mut Option<BoxStream<'static, ChangeBatch>>) -> Option<ChangeBatch> {
    match changes {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use application::ProviderRegistry;
    use application::ports::MountResolver;

    use crate::services::FileOperations;
    use application::testing::{FakeVfs, InstantClock};
    use application::use_cases::{CoalesceWindow, ListDirectory, WatchDirectory};

    const EVENT_DEADLINE: Duration = Duration::from_secs(5);

    fn services(fake: &FakeVfs) -> AppServices {
        let registry: Arc<dyn MountResolver> =
            Arc::new(ProviderRegistry::default().with_provider(fake.clone().into_factory()).unwrap());
        AppServices::new(
            Arc::new(ListDirectory::new(registry.clone())),
            Arc::new(WatchDirectory::new(registry.clone(), Arc::new(InstantClock), CoalesceWindow::default())),
            FileOperations::new(&registry),
            tokio::runtime::Handle::current(),
            fake.path("/"),
        )
    }

    fn recording_sink() -> (EventSink, Arc<Mutex<Vec<SessionEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = events.clone();
        (Box::new(move |event| sink_events.lock().unwrap().push(event)), events)
    }

    async fn wait_until(events: &Mutex<Vec<SessionEvent>>, predicate: impl Fn(&[SessionEvent]) -> bool) {
        tokio::time::timeout(EVENT_DEADLINE, async {
            while !predicate(&events.lock().unwrap()) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("expected session events did not arrive");
    }

    fn last_reset_len(events: &[SessionEvent]) -> Option<usize> {
        events.iter().rev().find_map(|e| match e {
            SessionEvent::Model(updates) => updates.iter().rev().find_map(|u| match u {
                ModelUpdate::Reset(rows) => Some(rows.len()),
                _ => None,
            }),
            _ => None,
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loads_then_applies_watched_changes() {
        let fake = FakeVfs::new();
        fake.add_dir("/d");
        fake.add_file("/d/a", 1);
        let (sink, events) = recording_sink();
        let config = SessionConfig { dir: fake.path("/d"), hidden: HiddenFiles::Hide, sort: SortSpec::default() };
        let _session = FolderSession::start(&services(&fake), config, sink);

        wait_until(&events, |e| matches!(e.last(), Some(SessionEvent::Loading(false)))).await;
        assert_eq!(last_reset_len(&events.lock().unwrap()), Some(1));

        fake.add_file("/d/b", 1);
        wait_until(&events, |e| {
            e.iter().any(|ev| matches!(ev, SessionEvent::Model(u) if matches!(u.as_slice(), [ModelUpdate::Insert { index: 1, .. }])))
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_folder_reports_failure() {
        let fake = FakeVfs::new();
        let (sink, events) = recording_sink();
        let config = SessionConfig { dir: fake.path("/missing"), hidden: HiddenFiles::Hide, sort: SortSpec::default() };
        let _session = FolderSession::start(&services(&fake), config, sink);
        wait_until(&events, |e| e.iter().any(|ev| matches!(ev, SessionEvent::Failed(VfsError::NotFound)))).await;
    }
}
