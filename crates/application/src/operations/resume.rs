//! What a transfer needs to survive a crash or quit (ADR 0019): it journals
//! large writes the destination can continue, and when it runs again for an
//! interrupted job it treats what the earlier run finished as done.

use std::sync::{Arc, Mutex, PoisonError};

use domain::{ByteSize, FileEntry, StoragePath};

use crate::ports::{InFlightWrite, JournalId, ResumeState, TransferJournal};

/// Smaller files simply start over: journaling them costs more than it saves.
pub const RESUMABLE_SIZE: ByteSize = ByteSize(16 * 1024 * 1024);

/// How a transfer treats items already at the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Existing {
    /// A fresh job: an existing item is a conflict for the user to resolve.
    Conflict,
    /// Continuing an interrupted job: folders are merged into, and files of
    /// the source's size are taken as copied by the earlier run.
    EarlierRun,
}

/// A partial write left by the interrupted run.
pub(crate) enum Previous {
    /// The source is unchanged: continue it.
    Continue(ResumeState),
    /// The source changed since: delete the partial data and start over.
    Stale(ResumeState),
}

pub(crate) struct Resumption {
    pub existing: Existing,
    journal: Option<(Arc<dyn TransferJournal>, JournalId)>,
    /// The write the interrupted run had in flight, until it's picked up.
    previous: Mutex<Option<InFlightWrite>>,
}

impl Resumption {
    pub(crate) const fn none() -> Self {
        Self { existing: Existing::Conflict, journal: None, previous: Mutex::new(None) }
    }

    pub(crate) fn journaled(journal: Arc<dyn TransferJournal>, id: JournalId) -> Self {
        Self { existing: Existing::Conflict, journal: Some((journal, id)), previous: Mutex::new(None) }
    }

    pub(crate) fn continuing(
        journal: Arc<dyn TransferJournal>,
        id: JournalId,
        previous: Option<InFlightWrite>,
    ) -> Self {
        Self { existing: Existing::EarlierRun, journal: Some((journal, id)), previous: Mutex::new(previous) }
    }

    pub(crate) fn is_journaled(&self) -> bool {
        self.journal.is_some()
    }

    /// What the interrupted run left for writing `target` from this source.
    pub(crate) fn take_previous(&self, source: &FileEntry, target: &StoragePath) -> Option<Previous> {
        let mut previous = self.previous.lock().unwrap_or_else(PoisonError::into_inner);
        let write = previous.as_ref()?;
        if write.source != *source.path() || write.target != *target {
            return None;
        }
        let unchanged =
            Some(write.source_size) == source.metadata().size && write.source_modified == source.metadata().modified;
        let state = previous.take()?.state;
        Some(if unchanged { Previous::Continue(state) } else { Previous::Stale(state) })
    }

    /// The write that was in flight but no longer matches its source; its
    /// leftovers should be discarded.
    pub(crate) fn take_stale(&self) -> Option<InFlightWrite> {
        self.previous.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// Journal failures never fail the transfer: resuming is best effort.
    pub(crate) async fn write_started(&self, write: InFlightWrite) {
        if let Some((journal, id)) = &self.journal {
            let _ = journal.checkpoint(*id, Some(write)).await;
        }
    }

    pub(crate) async fn write_finished(&self) {
        if let Some((journal, id)) = &self.journal {
            let _ = journal.checkpoint(*id, None).await;
        }
    }
}

/// A destination item counts as finished by the earlier run when it has the
/// source's kind and, for files, its size (writes are atomic, so a file of
/// the right size at the target is complete).
pub(crate) fn finished_earlier(source: &FileEntry, existing: &FileEntry) -> bool {
    source.kind() == existing.kind()
        && (source.kind() == domain::EntryKind::Directory || source.metadata().size == existing.metadata().size)
}
