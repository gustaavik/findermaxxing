use std::fs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use application::ports::Watcher;
use domain::{ChangeEvent, PathSegment, StoragePath, VfsError};
use futures::Stream;
use futures::channel::mpsc;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::error::from_io;
use crate::lister::{LocalFs, stat_blocking};
use crate::native_path::to_native;

impl Watcher for LocalFs {
    fn watch(&self, dir: &StoragePath) -> Result<impl Stream<Item = ChangeEvent> + Send + 'static, VfsError> {
        let native = to_native(dir)?;
        let metadata = fs::metadata(&native).map_err(|e| from_io(&e))?;
        if !metadata.is_dir() {
            return Err(VfsError::NotADirectory);
        }
        // FSEvents reports canonical paths (/private/var/… for /var/…), so
        // events are matched against the canonical form of the watched dir.
        let canonical = fs::canonicalize(&native).map_err(|e| from_io(&e))?;
        let (sender, events) = mpsc::unbounded();
        let translator = EventTranslator { dir: dir.clone(), canonical: canonical.clone(), sender };
        let mut watcher = notify::recommended_watcher(move |result| translator.handle(result))
            .map_err(|e| VfsError::Io { message: e.to_string() })?;
        watcher.watch(&canonical, RecursiveMode::NonRecursive).map_err(|e| VfsError::Io { message: e.to_string() })?;
        Ok(WatchStream { _watcher: watcher, events })
    }
}

/// Turns raw notify events into domain events. Runs on notify's own thread.
struct EventTranslator {
    dir: StoragePath,
    canonical: PathBuf,
    sender: mpsc::UnboundedSender<ChangeEvent>,
}

impl EventTranslator {
    fn handle(&self, result: notify::Result<Event>) {
        let events = match result {
            Ok(event) if event.need_rescan() => vec![ChangeEvent::Rescan],
            Ok(event) => event.paths.iter().filter_map(|p| self.translate(p)).collect(),
            Err(_) => vec![ChangeEvent::Rescan],
        };
        for event in events {
            // A closed channel means the view stopped watching; the watcher is dropped with it.
            let _ = self.sender.unbounded_send(event);
        }
    }

    /// Re-stats the path rather than trusting event flags, which FSEvents
    /// coalesces (one event can carry both "created" and "removed").
    fn translate(&self, changed: &Path) -> Option<ChangeEvent> {
        if changed.parent() != Some(self.canonical.as_path()) {
            return None;
        }
        let name = PathSegment::new(changed.file_name()?.to_str()?).ok()?;
        let path = self.dir.join(name);
        match stat_blocking(&path) {
            Ok(entry) => Some(ChangeEvent::Upserted(entry)),
            Err(VfsError::NotFound) => Some(ChangeEvent::Removed(path)),
            Err(_) => None,
        }
    }
}

/// Owns the OS watcher; dropping the stream unregisters it.
struct WatchStream {
    _watcher: RecommendedWatcher,
    events: mpsc::UnboundedReceiver<ChangeEvent>,
}

impl Stream for WatchStream {
    type Item = ChangeEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<ChangeEvent>> {
        Pin::new(&mut self.events).poll_next(cx)
    }
}
