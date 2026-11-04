use std::sync::Arc;

use domain::{FileCategory, FileEntry, StoragePath, VfsError};
use futures::TryStreamExt;

use crate::cancel::CancelToken;
use crate::ports::{
    ArchiveInspector, ArchiveListing, LocalFile, MountHandle, MountResolver, PreviewTheme, RenderedText, TextRenderer,
};

/// Archive previews list at most this many items.
pub const ARCHIVE_PREVIEW_LIMIT: usize = 500;
/// Unknown file types are tried as text up to this size.
const TEXT_SNIFF_LIMIT: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewBody {
    Image(LocalFile),
    Pdf(LocalFile),
    Video(LocalFile),
    Audio(LocalFile),
    Text(RenderedText),
    Archive(ArchiveListing),
    Folder {
        items: u64,
    },
    /// Metadata only (unknown or binary formats, or rendering failed).
    Summary {
        note: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub entry: FileEntry,
    pub category: FileCategory,
    pub body: PreviewBody,
}

/// Decides how to show an item and gathers what that needs.
pub struct PreparePreview {
    mounts: Arc<dyn MountResolver>,
    text: Arc<dyn TextRenderer>,
    archives: Arc<dyn ArchiveInspector>,
}

impl PreparePreview {
    #[must_use]
    pub fn new(
        mounts: Arc<dyn MountResolver>,
        text: Arc<dyn TextRenderer>,
        archives: Arc<dyn ArchiveInspector>,
    ) -> Self {
        Self { mounts, text, archives }
    }

    /// # Errors
    /// Provider errors reading the item itself. Rendering problems are not
    /// errors: they degrade to a [`PreviewBody::Summary`].
    pub async fn execute(&self, item: &StoragePath, theme: PreviewTheme) -> Result<Preview, VfsError> {
        let mount = self.mounts.resolve(item.mount())?;
        let entry = mount.lister()?.stat_boxed(item).await?;
        let category = FileCategory::of(&entry);
        let body = match category {
            FileCategory::Folder => folder_body(&mount, item).await,
            FileCategory::Image => local(&mount, item).await.map_or_else(|e| summary_of(&e), PreviewBody::Image),
            FileCategory::Pdf => local(&mount, item).await.map_or_else(|e| summary_of(&e), PreviewBody::Pdf),
            FileCategory::Video => local(&mount, item).await.map_or_else(|e| summary_of(&e), PreviewBody::Video),
            FileCategory::Audio => local(&mount, item).await.map_or_else(|e| summary_of(&e), PreviewBody::Audio),
            FileCategory::Text => self.text_body(&mount, item, theme).await,
            FileCategory::Archive => self.archive_body(&mount, item).await,
            FileCategory::Other if entry.metadata().size.is_some_and(|size| size.0 <= TEXT_SNIFF_LIMIT) => {
                self.text_body(&mount, item, theme).await
            }
            FileCategory::Document | FileCategory::Other => PreviewBody::Summary { note: None },
        };
        Ok(Preview { entry, category, body })
    }

    async fn text_body(&self, mount: &MountHandle, item: &StoragePath, theme: PreviewTheme) -> PreviewBody {
        let file = match local(mount, item).await {
            Ok(file) => file,
            Err(error) => return summary_of(&error),
        };
        match self.text.render(&file, theme).await {
            Ok(Some(text)) => PreviewBody::Text(text),
            Ok(None) => PreviewBody::Summary { note: None },
            Err(failure) => PreviewBody::Summary { note: Some(failure.message) },
        }
    }

    async fn archive_body(&self, mount: &MountHandle, item: &StoragePath) -> PreviewBody {
        let file = match local(mount, item).await {
            Ok(file) => file,
            Err(error) => return summary_of(&error),
        };
        match self.archives.list(&file, ARCHIVE_PREVIEW_LIMIT).await {
            Ok(listing) => PreviewBody::Archive(listing),
            Err(failure) => PreviewBody::Summary { note: Some(failure.message) },
        }
    }
}

async fn local(mount: &MountHandle, item: &StoragePath) -> Result<LocalFile, VfsError> {
    mount.materializer()?.local_file_boxed(item).await
}

fn summary_of(error: &VfsError) -> PreviewBody {
    PreviewBody::Summary { note: Some(error.to_string()) }
}

async fn folder_body(mount: &MountHandle, item: &StoragePath) -> PreviewBody {
    let listing = match mount.lister() {
        Ok(lister) => lister.list_boxed(item, CancelToken::never()).try_collect::<Vec<_>>().await,
        Err(error) => Err(error),
    };
    match listing {
        Ok(batches) => PreviewBody::Folder { items: batches.iter().map(Vec::len).sum::<usize>() as u64 },
        Err(error) => summary_of(&error),
    }
}

impl std::fmt::Debug for PreparePreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparePreview").finish_non_exhaustive()
    }
}
