//! The utilities (ADR 0027, 0028): hashing, archives, image tools and the
//! terminal, wired to the window's file operations so their changes undo.

use std::path::PathBuf;
use std::sync::Arc;

use adapter_archives::LocalArchiver;
use adapter_checksums::StandardHasher;
use adapter_cloud_models::HttpOrganizationModel;
use adapter_content_extract::LocalExtractor;
use adapter_imaging::LocalImageTools;
use adapter_terminal::AlacrittyLauncher;
use application::ports::{CloudModelStore, ContentExtractor, CredentialStore, MountResolver, UtilityTools};
use application::use_cases::{CloudParts, OrganizeParts};
use domain::ClutterPolicy;
use presentation::{FileOperations, Organize, Utilities};

use crate::indexing::LanguageModel;
use tokio::runtime::Handle;

pub(crate) fn build(
    runtime: &Handle,
    mounts: &Arc<dyn MountResolver>,
    operations: &FileOperations,
    home: PathBuf,
) -> Utilities {
    let archiver = Arc::new(LocalArchiver::new(runtime.clone()));
    let images = Arc::new(LocalImageTools::new(runtime.clone()));
    let tools = UtilityTools {
        hasher: Arc::new(StandardHasher),
        extractor: archiver.clone(),
        builder: archiver,
        converter: images.clone(),
        fingerprinter: images.clone(),
        metadata: images,
    };
    Utilities::new(mounts, operations, &tools, Arc::new(AlacrittyLauncher), home)
}

/// Organization suggestions (ADR 0029): the shared language model, the
/// utilities' duplicate finder and image metadata, and the cloud model.
pub(crate) fn organize(
    runtime: &Handle,
    mounts: &Arc<dyn MountResolver>,
    operations: &FileOperations,
    utilities: &Utilities,
    language: LanguageModel,
    cloud_models: Arc<dyn CloudModelStore>,
    credentials: Arc<dyn CredentialStore>,
) -> Result<Organize, reqwest::Error> {
    let extractor: Arc<dyn ContentExtractor> = Arc::new(LocalExtractor::new(runtime.clone()));
    let parts = OrganizeParts {
        mounts: mounts.clone(),
        extractor: extractor.clone(),
        text: language.text,
        installer: language.installer,
        image_metadata: Arc::new(LocalImageTools::new(runtime.clone())),
        duplicates: utilities.duplicates.clone(),
        cloud: CloudParts { store: cloud_models, credentials, model: Arc::new(HttpOrganizationModel::new()?) },
        policy: ClutterPolicy::default(),
    };
    Ok(Organize::new(parts, operations, extractor))
}
