//! Ports of the M7 utilities (ADR 0027): hashing, archives, image tools, and
//! two provider roles, permission editing and secure erasing.
//!
//! Archives and images are worked on as local files (`Materializer`), and
//! their results flow back as bytes, so the use cases can write them to any
//! provider through `Writer`.

use std::sync::Arc;

use domain::{
    ArchiveFormat, ByteSize, ConversionRequest, Digest, HashAlgorithm, ImageFingerprint, MetadataField, PathSegment,
    StoragePath, Timestamp, UnixMode, UnsafeEntry, VfsError,
};
use futures::future::BoxFuture;
use futures::stream::BoxStream;

use super::mutation::ProgressCallback;
use super::preview::LocalFile;
use crate::cancel::CancelToken;

/// Feeds bytes into a hash and yields the digest.
pub trait RunningHash: Send {
    fn update(&mut self, bytes: &[u8]);
    fn finish(self: Box<Self>) -> Digest;
}

pub trait ContentHasher: Send + Sync {
    fn start(&self, algorithm: HashAlgorithm) -> Box<dyn RunningHash>;
}

/// One step of an archive's contents, in order. A file is `File`, its
/// `Chunk`s, then `EndFile`. Paths are relative to the archive root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveEvent {
    Folder(Vec<PathSegment>),
    File {
        path: Vec<PathSegment>,
        size: Option<ByteSize>,
        mode: Option<UnixMode>,
        modified: Option<Timestamp>,
    },
    Chunk(Vec<u8>),
    EndFile,
    /// An entry left out (links, devices, unsafe paths); reported, not fatal.
    Skipped {
        name: String,
        reason: SkipReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Unsafe(UnsafeEntry),
    /// Links and special files are not recreated.
    NotARegularFile,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArchiveError {
    #[error("the archive is password-protected")]
    Encrypted,
    #[error("the archive is damaged or not a {0:?} archive: {1}")]
    Corrupt(ArchiveFormat, String),
    #[error("{0:?} archives can't be created")]
    CannotCreate(ArchiveFormat),
    #[error(transparent)]
    Io(#[from] VfsError),
}

pub trait ArchiveExtractor: Send + Sync {
    /// The archive's contents as events. Dropping the stream stops reading.
    fn extract(
        &self,
        archive: LocalFile,
        format: ArchiveFormat,
        cancel: CancelToken,
    ) -> BoxStream<'static, Result<ArchiveEvent, ArchiveError>>;
}

pub trait ArchiveBuilder: Send + Sync {
    /// Packs `contents` (folders, files and chunks; never `Skipped`) and
    /// yields the finished archive's bytes once all contents are in.
    fn build(
        &self,
        format: ArchiveFormat,
        contents: BoxStream<'static, ArchiveEvent>,
        cancel: CancelToken,
    ) -> BoxStream<'static, Result<Vec<u8>, ArchiveError>>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    #[error("this image format isn't supported")]
    Unsupported,
    #[error("the image couldn't be read: {0}")]
    Unreadable(String),
    #[error("the image couldn't be written: {0}")]
    Encoding(String),
}

pub trait ImageConverter: Send + Sync {
    /// The converted image's bytes.
    fn convert(&self, image: LocalFile, request: ConversionRequest) -> BoxFuture<'static, Result<Vec<u8>, ImageError>>;
}

pub trait ImageFingerprinter: Send + Sync {
    /// `None` for files that aren't decodable images.
    fn fingerprint(&self, image: LocalFile) -> BoxFuture<'static, Option<ImageFingerprint>>;
}

pub trait ImageMetadata: Send + Sync {
    /// EXIF fields; empty when there are none.
    fn read(&self, image: LocalFile) -> BoxFuture<'static, Result<Vec<MetadataField>, ImageError>>;

    /// The same image without EXIF, XMP and comments (colour profiles stay,
    /// pixels are untouched).
    fn strip(&self, image: LocalFile) -> BoxFuture<'static, Result<Vec<u8>, ImageError>>;
}

/// Provider role: change POSIX permissions.
pub trait PermissionEditor: Send + Sync {
    fn set_permissions<'a>(&'a self, path: &'a StoragePath, mode: UnixMode) -> BoxFuture<'a, Result<(), VfsError>>;
}

/// Provider role: overwrite a file's contents, then remove it.
pub trait SecureEraser: Send + Sync {
    /// Files only; `progress` reports overwritten bytes.
    fn erase<'a>(
        &'a self,
        file: &'a StoragePath,
        passes: u8,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> BoxFuture<'a, Result<(), VfsError>>;
}

/// Shared handles the utilities need, built once by the composition root.
#[derive(Clone)]
pub struct UtilityTools {
    pub hasher: Arc<dyn ContentHasher>,
    pub extractor: Arc<dyn ArchiveExtractor>,
    pub builder: Arc<dyn ArchiveBuilder>,
    pub converter: Arc<dyn ImageConverter>,
    pub fingerprinter: Arc<dyn ImageFingerprinter>,
    pub metadata: Arc<dyn ImageMetadata>,
}

impl std::fmt::Debug for UtilityTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UtilityTools").finish_non_exhaustive()
    }
}
