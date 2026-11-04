use std::sync::Arc;

use application::use_cases::{ListDirectory, WatchDirectory};
use domain::StoragePath;
use tokio::runtime::Handle;

/// Everything bridge objects need, built by the composition root and passed
/// down the QML tree explicitly (no globals): `main` → root window property →
/// each `FolderViewController.services`.
#[derive(Debug, Clone)]
pub struct AppServices {
    pub(crate) list_directory: Arc<ListDirectory>,
    pub(crate) watch_directory: Arc<WatchDirectory>,
    pub(crate) runtime: Handle,
    pub(crate) home: StoragePath,
}

impl AppServices {
    #[must_use]
    pub fn new(
        list_directory: Arc<ListDirectory>,
        watch_directory: Arc<WatchDirectory>,
        runtime: Handle,
        home: StoragePath,
    ) -> Self {
        Self { list_directory, watch_directory, runtime, home }
    }
}
