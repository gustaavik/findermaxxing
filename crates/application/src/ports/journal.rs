//! A durable record of running copy and move jobs, so that a job cut short by
//! a crash or by quitting can be resumed on the next start.

use domain::{ByteSize, StoragePath, Timestamp};
use futures::future::BoxFuture;

use super::saved_searches::StoreError;
use super::stream::ResumeState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JournalId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournaledKind {
    Copy,
    Move,
}

/// A large file write that the destination can continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightWrite {
    pub source: StoragePath,
    pub target: StoragePath,
    /// The source as it was when the write started; a changed source restarts.
    pub source_size: ByteSize,
    pub source_modified: Option<Timestamp>,
    pub state: ResumeState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub kind: JournaledKind,
    pub sources: Vec<StoragePath>,
    pub destination: StoragePath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedTransfer {
    pub id: JournalId,
    pub entry: JournalEntry,
    pub in_flight: Option<InFlightWrite>,
}

pub trait TransferJournal: Send + Sync {
    fn begin(&self, entry: JournalEntry) -> BoxFuture<'static, Result<JournalId, StoreError>>;

    /// Records (or, with `None`, clears) the write that can be continued.
    fn checkpoint(&self, id: JournalId, write: Option<InFlightWrite>) -> BoxFuture<'static, Result<(), StoreError>>;

    /// The job ended (finished, failed or cancelled): nothing to resume.
    fn finish(&self, id: JournalId) -> BoxFuture<'static, Result<(), StoreError>>;

    /// Jobs that never finished, oldest first.
    fn interrupted(&self) -> BoxFuture<'static, Result<Vec<InterruptedTransfer>, StoreError>>;
}
