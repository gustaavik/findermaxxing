//! Organization suggestions (ADR 0029): analyse a folder, show the user a
//! plan, apply what they accept as one undoable step.

mod analyze;
mod apply;
mod cloud;
mod gather;

use std::sync::Arc;

use domain::{ClutterPolicy, VfsError};

pub use analyze::{Analysis, AnalyzeFolder, CloudUse, clutter_text};
pub use apply::ApplySuggestions;
pub use cloud::{CloudModelSetup, CloudModelStatus, CloudParts, PreviewCloudRequest};
pub use gather::EXCERPT_CHARS;

use crate::ports::{
    ContentExtractor, CredentialError, ImageMetadata, ModelError, ModelInstaller, MountResolver, StoreError,
    TextEmbedder,
};
use crate::use_cases::FindDuplicates;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrganizeError {
    #[error(transparent)]
    Vfs(#[from] VfsError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Keychain(#[from] CredentialError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("no cloud model is set up")]
    NoCloudModel,
}

/// What analysing needs.
#[derive(Clone)]
pub struct OrganizeParts {
    pub mounts: Arc<dyn MountResolver>,
    pub extractor: Arc<dyn ContentExtractor>,
    pub text: Arc<dyn TextEmbedder>,
    pub installer: Arc<dyn ModelInstaller>,
    pub image_metadata: Arc<dyn ImageMetadata>,
    pub duplicates: Arc<FindDuplicates>,
    pub cloud: CloudParts,
    pub policy: ClutterPolicy,
}

impl std::fmt::Debug for OrganizeParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrganizeParts").finish_non_exhaustive()
    }
}
