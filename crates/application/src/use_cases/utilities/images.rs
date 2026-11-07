//! Converting and resizing images, and viewing and removing their metadata.
//! Conversions are written next to the originals (or into a chosen folder);
//! cleaning replaces the original, which goes to the Trash, so it's undoable.

use std::sync::Arc;

use domain::{
    ConversionRequest, ImageFormat, MetadataField, Operation, OperationKind, PathSegment, StoragePath, converted_name,
};

use super::checksums::opaque_debug;
use super::support::{local_copy, no_progress, once, write_new_file};
use crate::operations::progress::ProgressTracker;
use crate::operations::{JobContext, OperationError, OperationHistory, OperationReport};
use crate::ports::{ImageConverter, ImageError, ImageMetadata, MountResolver};

/// Prefix of the hidden file a cleaned image is written to before it takes
/// the original's place.
const CLEANING_PREFIX: &str = ".fm-cleaning-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertRequest {
    pub images: Vec<StoragePath>,
    pub conversion: ConversionRequest,
    /// `None`: next to each original.
    pub destination: Option<StoragePath>,
}

pub struct ConvertImages {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    converter: Arc<dyn ImageConverter>,
}

impl ConvertImages {
    #[must_use]
    pub fn new(
        mounts: Arc<dyn MountResolver>,
        history: Arc<OperationHistory>,
        converter: Arc<dyn ImageConverter>,
    ) -> Self {
        Self { mounts, history, converter }
    }

    /// Writes `photo.png` (or `photo 2.png`) for each image; originals stay.
    pub async fn execute(&self, request: &ConvertRequest, ctx: &JobContext) -> OperationReport {
        let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
        tracker.begin(request.images.len() as u64, domain::ByteSize(0));
        let mut created = Vec::new();
        let mut result = Ok(());
        for image in &request.images {
            if ctx.cancel.is_cancelled() {
                result = Err(OperationError::Cancelled);
                break;
            }
            tracker.item_started(image.name());
            match self.convert_one(image, request, ctx).await {
                Ok(path) => created.push(path),
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
            tracker.item_finished();
        }
        let done = Operation::Created(created);
        self.history.record(OperationKind::ConvertImages, done.clone());
        OperationReport::new(done, result)
    }

    async fn convert_one(
        &self,
        image: &StoragePath,
        request: &ConvertRequest,
        ctx: &JobContext,
    ) -> Result<StoragePath, OperationError> {
        let (Some(name), Some(parent)) = (image.name(), image.parent()) else {
            return Err(OperationError::WrongKind(image.clone()));
        };
        let local = local_copy(&*self.mounts, image).await.map_err(OperationError::on(image))?;
        let bytes = self.converter.convert(local, request.conversion).await.map_err(OperationError::Image)?;
        let folder = request.destination.clone().unwrap_or(parent);
        let mount = self.mounts.resolve(folder.mount()).map_err(OperationError::on(&folder))?;
        let new_name =
            converted_name(name, request.conversion.format).map_err(|e| OperationError::on(image)(e.into()))?;
        write_new_file(&mount, &folder, &new_name, once(bytes), &no_progress(), &ctx.cancel)
            .await
            .map_err(OperationError::on(&folder))
    }
}

pub struct ReadImageMetadata {
    mounts: Arc<dyn MountResolver>,
    metadata: Arc<dyn ImageMetadata>,
}

impl ReadImageMetadata {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, metadata: Arc<dyn ImageMetadata>) -> Self {
        Self { mounts, metadata }
    }

    /// # Errors
    /// The image couldn't be fetched or read.
    pub async fn execute(&self, image: &StoragePath) -> Result<Vec<MetadataField>, OperationError> {
        let local = local_copy(&*self.mounts, image).await.map_err(OperationError::on(image))?;
        self.metadata.read(local).await.map_err(OperationError::Image)
    }
}

pub struct CleanImageMetadata {
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    metadata: Arc<dyn ImageMetadata>,
}

impl CleanImageMetadata {
    #[must_use]
    pub fn new(
        mounts: Arc<dyn MountResolver>,
        history: Arc<OperationHistory>,
        metadata: Arc<dyn ImageMetadata>,
    ) -> Self {
        Self { mounts, history, metadata }
    }

    /// Replaces each image with a copy without metadata; the originals go
    /// to the Trash. Needs a Trash, so it can always be undone.
    pub async fn execute(&self, images: &[StoragePath], ctx: &JobContext) -> OperationReport {
        let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
        tracker.begin(images.len() as u64, domain::ByteSize(0));
        let mut done = Vec::new();
        let mut result = Ok(());
        for image in images {
            if ctx.cancel.is_cancelled() {
                result = Err(OperationError::Cancelled);
                break;
            }
            tracker.item_started(image.name());
            match self.clean_one(image, ctx).await {
                Ok(operation) => done.push(operation),
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
            tracker.item_finished();
        }
        let done = Operation::sequence(done);
        self.history.record(OperationKind::CleanMetadata, done.clone());
        OperationReport::new(done, result)
    }

    async fn clean_one(&self, image: &StoragePath, ctx: &JobContext) -> Result<Operation, OperationError> {
        let (Some(name), Some(folder)) = (image.name(), image.parent()) else {
            return Err(OperationError::WrongKind(image.clone()));
        };
        if !ImageFormat::of(name).is_some_and(ImageFormat::can_clean_metadata) {
            return Err(OperationError::Image(ImageError::Unsupported));
        }
        let on_image = OperationError::on(image);
        let mount = self.mounts.resolve(image.mount()).map_err(OperationError::on(image))?;
        let trasher = mount.trasher().map_err(OperationError::on(image))?;
        let mover = mount.mover().map_err(OperationError::on(image))?;
        let local = local_copy(&*self.mounts, image).await.map_err(OperationError::on(image))?;
        let cleaned = self.metadata.strip(local).await.map_err(OperationError::Image)?;
        let staging_name = PathSegment::new(&format!("{CLEANING_PREFIX}{}", name.display_name()))
            .map_err(|e| OperationError::on(image)(e.into()))?;
        let staging = write_new_file(&mount, &folder, &staging_name, once(cleaned), &no_progress(), &ctx.cancel)
            .await
            .map_err(OperationError::on(image))?;
        let ticket = match trasher.trash_boxed(image).await {
            Ok(ticket) => ticket,
            Err(error) => {
                if let Ok(remover) = mount.remover() {
                    let _ = remover.remove_boxed(&staging, ctx.cancel.clone()).await;
                }
                return Err(on_image(error));
            }
        };
        match mover.rename_boxed(&staging, image).await {
            Ok(()) => {
                Ok(Operation::sequence(vec![Operation::Trashed(vec![ticket]), Operation::Created(vec![image.clone()])]))
            }
            Err(error) => {
                // Put the original back; the cleaned copy stays under its staging name.
                let _ = trasher.restore_boxed(&ticket).await;
                Err(on_image(error))
            }
        }
    }
}

opaque_debug!(ConvertImages, ReadImageMetadata, CleanImageMetadata);
