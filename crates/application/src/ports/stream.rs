//! Streaming roles: what makes copies between *different* providers possible.
//!
//! Contract (verified by `stream_contract_tests!`):
//! - `create` fails with `AlreadyExists` if the target exists; nothing is
//!   visible at the target until `commit` succeeds;
//! - `abort` (or dropping an uncommitted sink) leaves nothing behind;
//! - `read` of a missing item is `NotFound`, of a folder `IsADirectory`.
//!
//! Resuming (ADR 0019): a sink that hands out a [`ResumeState`] through
//! `checkpoint` keeps its partial data when dropped uncommitted, so that
//! `Writer::resume` can continue it after a crash or quit. `abort` still
//! removes it. `Writer::discard` removes the leftovers of a state for good.

use std::future::Future;

use domain::{ByteSize, StoragePath, VfsError};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt};

use crate::cancel::CancelToken;

pub type ByteStream = BoxStream<'static, Result<Vec<u8>, VfsError>>;

/// Provider-specific handle on a partial write (a multipart upload id, the
/// name of a partial file). Opaque to everything but the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeState(pub String);

/// A sink continuing a partial write: the next byte written lands at `offset`.
pub struct ResumedSink {
    pub sink: Box<dyn WriteSink>,
    pub offset: ByteSize,
}

impl std::fmt::Debug for ResumedSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumedSink").field("offset", &self.offset).finish_non_exhaustive()
    }
}

pub trait Reader: Send + Sync + 'static {
    /// Opens an item for sequential reading. Open errors surface here, not mid-stream.
    fn read(
        &self,
        path: &StoragePath,
        cancel: CancelToken,
    ) -> impl Future<Output = Result<ByteStream, VfsError>> + Send;

    /// Reads from `offset` on. The default reads from the start and drops the
    /// first `offset` bytes; providers that can seek override it.
    fn read_from(
        &self,
        path: &StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> impl Future<Output = Result<ByteStream, VfsError>> + Send {
        let opened = self.read(path, cancel);
        async move { Ok(skip_bytes(opened.await?, offset)) }
    }
}

/// Drops the first `count` bytes of `stream`.
pub fn skip_bytes(stream: ByteStream, count: ByteSize) -> ByteStream {
    stream
        .scan(count.0, |remaining, chunk| {
            let chunk = chunk.map(|mut bytes| {
                let skipped = usize::try_from(*remaining).unwrap_or(usize::MAX).min(bytes.len());
                *remaining -= skipped as u64;
                bytes.drain(..skipped);
                bytes
            });
            futures::future::ready(Some(chunk))
        })
        .filter(|chunk| futures::future::ready(!matches!(chunk, Ok(bytes) if bytes.is_empty())))
        .boxed()
}

/// An in-progress write. Exactly one of `commit` or `abort` should be called.
pub trait WriteSink: Send {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>>;

    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>>;

    fn abort(self: Box<Self>) -> BoxFuture<'static, ()>;

    /// A handle for continuing this write later, if the provider can. From
    /// then on, dropping the sink uncommitted keeps the partial data.
    fn checkpoint(&mut self) -> Option<ResumeState> {
        None
    }
}

pub trait Writer: Send + Sync + 'static {
    /// `size_hint` lets providers pick a strategy (single PUT vs. multipart).
    fn create(
        &self,
        path: &StoragePath,
        size_hint: Option<ByteSize>,
    ) -> impl Future<Output = Result<Box<dyn WriteSink>, VfsError>> + Send;

    /// Continues the write `state` describes. `Err(NotFound)` when the partial
    /// data is gone (the caller then starts over).
    fn resume(
        &self,
        path: &StoragePath,
        state: &ResumeState,
    ) -> impl Future<Output = Result<ResumedSink, VfsError>> + Send {
        let _ = (path, state);
        async { Err(VfsError::Unsupported(domain::Capability::ResumableUpload)) }
    }

    /// Deletes whatever `state` left behind. Missing leftovers are not an error.
    fn discard(&self, path: &StoragePath, state: &ResumeState) -> impl Future<Output = Result<(), VfsError>> + Send {
        let _ = (path, state);
        async { Ok(()) }
    }
}

pub trait DynReader: Send + Sync {
    fn read_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<ByteStream, VfsError>>;

    fn read_from_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<ByteStream, VfsError>>;
}

impl<T: Reader> DynReader for T {
    fn read_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<ByteStream, VfsError>> {
        self.read(path, cancel).boxed()
    }

    fn read_from_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        offset: ByteSize,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<ByteStream, VfsError>> {
        self.read_from(path, offset, cancel).boxed()
    }
}

pub trait DynWriter: Send + Sync {
    fn create_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        size_hint: Option<ByteSize>,
    ) -> BoxFuture<'a, Result<Box<dyn WriteSink>, VfsError>>;

    fn resume_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        state: &'a ResumeState,
    ) -> BoxFuture<'a, Result<ResumedSink, VfsError>>;

    fn discard_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        state: &'a ResumeState,
    ) -> BoxFuture<'a, Result<(), VfsError>>;
}

impl<T: Writer> DynWriter for T {
    fn create_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        size_hint: Option<ByteSize>,
    ) -> BoxFuture<'a, Result<Box<dyn WriteSink>, VfsError>> {
        self.create(path, size_hint).boxed()
    }

    fn resume_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        state: &'a ResumeState,
    ) -> BoxFuture<'a, Result<ResumedSink, VfsError>> {
        self.resume(path, state).boxed()
    }

    fn discard_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        state: &'a ResumeState,
    ) -> BoxFuture<'a, Result<(), VfsError>> {
        self.discard(path, state).boxed()
    }
}
