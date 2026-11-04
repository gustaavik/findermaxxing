//! `TransferJournal`: copy and move jobs that haven't finished yet.

use application::ports::{
    InFlightWrite, InterruptedTransfer, JournalEntry, JournalId, JournaledKind, ResumeState, StoreError,
    TransferJournal,
};
use domain::{ByteSize, Timestamp};
use futures::future::BoxFuture;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::location_codec::{corrupt, decode_location, decode_locations, encode_locations};
use crate::{SqliteStore, unavailable};

const COPY: &str = "copy";
const MOVE: &str = "move";

#[derive(Serialize, Deserialize)]
struct StoredWrite {
    source: String,
    target: String,
    source_size: u64,
    source_modified: Option<i64>,
    state: String,
}

fn encode_write(write: &InFlightWrite) -> Result<String, StoreError> {
    serde_json::to_string(&StoredWrite {
        source: write.source.encode(),
        target: write.target.encode(),
        source_size: write.source_size.0,
        source_modified: write.source_modified.map(Timestamp::unix_millis),
        state: write.state.0.clone(),
    })
    .map_err(corrupt)
}

fn decode_write(json: &str) -> Result<InFlightWrite, StoreError> {
    let stored: StoredWrite = serde_json::from_str(json).map_err(corrupt)?;
    Ok(InFlightWrite {
        source: decode_location(&stored.source)?,
        target: decode_location(&stored.target)?,
        source_size: ByteSize(stored.source_size),
        source_modified: stored.source_modified.map(Timestamp::from_unix_millis),
        state: ResumeState(stored.state),
    })
}

impl TransferJournal for SqliteStore {
    fn begin(&self, entry: JournalEntry) -> BoxFuture<'static, Result<JournalId, StoreError>> {
        self.run(move |connection| {
            let kind = match entry.kind {
                JournaledKind::Copy => COPY,
                JournaledKind::Move => MOVE,
            };
            connection
                .execute(
                    "INSERT INTO transfer_journal (kind, sources, destination) VALUES (?1, ?2, ?3)",
                    params![kind, encode_locations(&entry.sources)?, entry.destination.encode()],
                )
                .map_err(|e| unavailable(&e))?;
            Ok(JournalId(connection.last_insert_rowid()))
        })
    }

    fn checkpoint(&self, id: JournalId, write: Option<InFlightWrite>) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            let encoded = write.as_ref().map(encode_write).transpose()?;
            connection
                .execute("UPDATE transfer_journal SET in_flight = ?2 WHERE id = ?1", params![id.0, encoded])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn finish(&self, id: JournalId) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            connection
                .execute("DELETE FROM transfer_journal WHERE id = ?1", [id.0])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn interrupted(&self) -> BoxFuture<'static, Result<Vec<InterruptedTransfer>, StoreError>> {
        self.run(|connection| {
            let mut statement = connection
                .prepare("SELECT id, kind, sources, destination, in_flight FROM transfer_journal ORDER BY id")
                .map_err(|e| unavailable(&e))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })
                .map_err(|e| unavailable(&e))?;
            rows.map(|row| {
                let (id, kind, sources, destination, in_flight) = row.map_err(|e| unavailable(&e))?;
                let kind = match kind.as_str() {
                    COPY => JournaledKind::Copy,
                    MOVE => JournaledKind::Move,
                    other => return Err(corrupt(format!("unknown transfer kind {other:?}"))),
                };
                Ok(InterruptedTransfer {
                    id: JournalId(id),
                    entry: JournalEntry {
                        kind,
                        sources: decode_locations(&sources)?,
                        destination: decode_location(&destination)?,
                    },
                    in_flight: in_flight.as_deref().map(decode_write).transpose()?,
                })
            })
            .collect()
        })
    }
}
