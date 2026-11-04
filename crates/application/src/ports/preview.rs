//! Ports used to build previews. Previews always work on a local file, which
//! the mount's [`Materializer`] provides (a cached download for remote
//! providers from M4 on).

use std::future::Future;
use std::path::{Path, PathBuf};

use domain::{ByteSize, StoragePath, VfsError};
use futures::FutureExt;
use futures::future::BoxFuture;

/// A file on the local disk that stands for an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile(PathBuf);

impl LocalFile {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

pub trait Materializer: Send + Sync + 'static {
    fn local_file(&self, item: &StoragePath) -> impl Future<Output = Result<LocalFile, VfsError>> + Send;
}

pub trait DynMaterializer: Send + Sync {
    fn local_file_boxed<'a>(&'a self, item: &'a StoragePath) -> BoxFuture<'a, Result<LocalFile, VfsError>>;
}

impl<T: Materializer> DynMaterializer for T {
    fn local_file_boxed<'a>(&'a self, item: &'a StoragePath) -> BoxFuture<'a, Result<LocalFile, VfsError>> {
        self.local_file(item).boxed()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewTheme {
    Light,
    Dark,
}

/// Syntax-highlighted text, as HTML with inline styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedText {
    pub html: String,
    pub language: String,
    /// Only the beginning of the file was rendered.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveItem {
    pub path: String,
    pub size: Option<ByteSize>,
    pub is_folder: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveListing {
    pub items: Vec<ArchiveItem>,
    /// More items exist than were listed.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PreviewFailure {
    pub message: String,
}

/// Renders the start of a text file. `Ok(None)` means the file is binary.
pub trait TextRenderer: Send + Sync {
    fn render(
        &self,
        file: &LocalFile,
        theme: PreviewTheme,
    ) -> BoxFuture<'static, Result<Option<RenderedText>, PreviewFailure>>;
}

/// Lists the contents of an archive without extracting it.
pub trait ArchiveInspector: Send + Sync {
    fn list(&self, file: &LocalFile, limit: usize) -> BoxFuture<'static, Result<ArchiveListing, PreviewFailure>>;
}
