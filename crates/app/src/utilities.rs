//! The utilities (ADR 0027, 0028): hashing, archives, image tools and the
//! terminal, wired to the window's file operations so their changes undo.

use std::path::PathBuf;
use std::sync::Arc;

use adapter_archives::LocalArchiver;
use adapter_checksums::StandardHasher;
use adapter_imaging::LocalImageTools;
use adapter_terminal::AlacrittyLauncher;
use application::ports::{MountResolver, UtilityTools};
use presentation::{FileOperations, Utilities};
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
