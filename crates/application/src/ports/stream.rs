//! Streaming roles: what makes copies between *different* providers possible.
//!
//! Contract (verified by `stream_contract_tests!`):
//! - `create` fails with `AlreadyExists` if the target exists; nothing is
//!   visible at the target until `commit` succeeds;
//! - `abort` (or dropping an uncommitted sink) leaves nothing behind;
//! - `read` of a missing item is `NotFound`, of a folder `IsADirectory`.

use std::future::Future;

use domain::{ByteSize, StoragePath, VfsError};
use futures::FutureExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;

use crate::cancel::CancelToken;

pub type ByteStream = BoxStream<'static, Result<Vec<u8>, VfsError>>;

pub trait Reader: Send + Sync + 'static {
    /// Opens an item for sequential reading. Open errors surface here, not mid-stream.
    fn read(
        &self,
        path: &StoragePath,
        cancel: CancelToken,
    ) -> impl Future<Output = Result<ByteStream, VfsError>> + Send;
}

/// An in-progress write. Exactly one of `commit` or `abort` should be called.
pub trait WriteSink: Send {
    fn write(&mut self, chunk: Vec<u8>) -> BoxFuture<'_, Result<(), VfsError>>;

    fn commit(self: Box<Self>) -> BoxFuture<'static, Result<(), VfsError>>;

    fn abort(self: Box<Self>) -> BoxFuture<'static, ()>;
}

pub trait Writer: Send + Sync + 'static {
    /// `size_hint` lets providers pick a strategy (single PUT vs. multipart).
    fn create(
        &self,
        path: &StoragePath,
        size_hint: Option<ByteSize>,
    ) -> impl Future<Output = Result<Box<dyn WriteSink>, VfsError>> + Send;
}

pub trait DynReader: Send + Sync {
    fn read_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
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
}

pub trait DynWriter: Send + Sync {
    fn create_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        size_hint: Option<ByteSize>,
    ) -> BoxFuture<'a, Result<Box<dyn WriteSink>, VfsError>>;
}

impl<T: Writer> DynWriter for T {
    fn create_boxed<'a>(
        &'a self,
        path: &'a StoragePath,
        size_hint: Option<ByteSize>,
    ) -> BoxFuture<'a, Result<Box<dyn WriteSink>, VfsError>> {
        self.create(path, size_hint).boxed()
    }
}
