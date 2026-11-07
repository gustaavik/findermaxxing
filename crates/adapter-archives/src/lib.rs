//! `ArchiveExtractor` and `ArchiveBuilder` (ADR 0027).
//!
//! - zip (zip crate: deflate, deflate64, bzip2, zstd, lzma);
//! - tar, plain or compressed with gzip, bzip2, xz or zstd;
//! - 7z (sevenz-rust2, LZMA2 when writing);
//! - RAR, extraction only, through RARLAB's unRAR (its licence forbids
//!   creating RAR archives).
//!
//! The libraries are blocking, so each job runs on tokio's blocking pool and
//! talks to the async side through small channels: back-pressure keeps
//! memory flat, and dropping the stream stops the work.

mod build;
mod extract;

use std::io;

use application::CancelToken;
use application::ports::{ArchiveBuilder, ArchiveError, ArchiveEvent, ArchiveExtractor, LocalFile};
use domain::{ArchiveFormat, VfsError};
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

/// Events or output chunks in flight between the two sides.
const CHANNEL_DEPTH: usize = 16;
/// Read size for entry data and finished archives.
pub(crate) const CHUNK: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct LocalArchiver {
    runtime: Handle,
}

impl LocalArchiver {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }
}

/// Sends from a blocking thread; `Err` once the async side has gone away.
pub(crate) struct Outbox<T> {
    sender: mpsc::Sender<Result<T, ArchiveError>>,
    cancel: CancelToken,
}

/// The receiving side hung up or the job was cancelled: stop quietly.
#[derive(Debug)]
pub(crate) struct Stop;

impl<T> Outbox<T> {
    pub(crate) fn send(&self, item: T) -> Result<(), Stop> {
        if self.cancel.is_cancelled() {
            return Err(Stop);
        }
        self.sender.blocking_send(Ok(item)).map_err(|_| Stop)
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    fn fail(&self, error: ArchiveError) {
        let _ = self.sender.blocking_send(Err(error));
    }
}

/// Why a blocking job ended early.
pub(crate) enum Failure {
    Stopped,
    Error(ArchiveError),
}

impl From<Stop> for Failure {
    fn from(_: Stop) -> Self {
        Self::Stopped
    }
}

impl From<ArchiveError> for Failure {
    fn from(error: ArchiveError) -> Self {
        Self::Error(error)
    }
}

pub(crate) fn corrupt(format: ArchiveFormat) -> impl Fn(io::Error) -> Failure {
    move |error| Failure::Error(ArchiveError::Corrupt(format, error.to_string()))
}

pub(crate) fn io_failure(error: &io::Error) -> Failure {
    Failure::Error(ArchiveError::Io(VfsError::Io { message: error.to_string() }))
}

fn receiver_stream<T: Send + 'static>(
    receiver: mpsc::Receiver<Result<T, ArchiveError>>,
) -> BoxStream<'static, Result<T, ArchiveError>> {
    futures::stream::unfold(receiver, |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) })
        .boxed()
}

fn run_blocking<T: Send + 'static>(
    runtime: &Handle,
    cancel: CancelToken,
    job: impl FnOnce(&Outbox<T>) -> Result<(), Failure> + Send + 'static,
) -> BoxStream<'static, Result<T, ArchiveError>> {
    let (sender, receiver) = mpsc::channel(CHANNEL_DEPTH);
    runtime.spawn_blocking(move || {
        let outbox = Outbox { sender, cancel };
        if let Err(Failure::Error(error)) = job(&outbox) {
            outbox.fail(error);
        }
    });
    receiver_stream(receiver)
}

impl ArchiveExtractor for LocalArchiver {
    fn extract(
        &self,
        archive: LocalFile,
        format: ArchiveFormat,
        cancel: CancelToken,
    ) -> BoxStream<'static, Result<ArchiveEvent, ArchiveError>> {
        run_blocking(&self.runtime, cancel, move |outbox| extract::extract(archive.path(), format, outbox))
    }
}

impl ArchiveBuilder for LocalArchiver {
    fn build(
        &self,
        format: ArchiveFormat,
        contents: BoxStream<'static, ArchiveEvent>,
        cancel: CancelToken,
    ) -> BoxStream<'static, Result<Vec<u8>, ArchiveError>> {
        if !format.can_create() {
            return futures::stream::once(async move { Err(ArchiveError::CannotCreate(format)) }).boxed();
        }
        // The async contents are handed to the blocking writer through a channel.
        let (inbox_sender, inbox) = mpsc::channel(CHANNEL_DEPTH);
        self.runtime.spawn(async move {
            let mut contents = contents;
            while let Some(event) = contents.next().await {
                if inbox_sender.send(event).await.is_err() {
                    break;
                }
            }
        });
        run_blocking(&self.runtime, cancel, move |outbox| build::build(format, inbox, outbox))
    }
}
