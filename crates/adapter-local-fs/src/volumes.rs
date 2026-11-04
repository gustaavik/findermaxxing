//! Mounted volumes for the sidebar: `/Volumes` on macOS; `/media/$USER`,
//! `/run/media/$USER` and `/mnt` on Linux.

use std::fs;
use std::path::{Path, PathBuf};

use application::ports::{Volume, VolumeLister};
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::runtime::Handle;

use crate::native_path::storage_path_from_native;

const HIDDEN_PREFIX: char = '.';

#[derive(Debug, Clone)]
pub struct LocalVolumes {
    runtime: Handle,
}

impl LocalVolumes {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }
}

impl VolumeLister for LocalVolumes {
    fn volumes(&self) -> BoxFuture<'static, Vec<Volume>> {
        let task = self.runtime.spawn_blocking(|| mount_roots().iter().flat_map(|root| volumes_in(root)).collect());
        async move { task.await.unwrap_or_default() }.boxed()
    }
}

#[cfg(target_os = "macos")]
fn mount_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/Volumes")]
}

#[cfg(not(target_os = "macos"))]
fn mount_roots() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    vec![PathBuf::from("/media").join(&user), PathBuf::from("/run/media").join(&user), PathBuf::from("/mnt")]
}

/// Folders directly inside `root`, skipping hidden ones (`.timemachine`) and
/// links back to the boot volume (macOS lists "Macintosh HD" in /Volumes as a
/// symlink to `/`).
fn volumes_in(root: &Path) -> Vec<Volume> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    let mut volumes: Vec<Volume> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with(HIDDEN_PREFIX))
        .filter_map(|entry| {
            let location = storage_path_from_native(&entry.path()).ok()?;
            Some(Volume { name: entry.file_name().to_string_lossy().into_owned(), location })
        })
        .collect();
    volumes.sort_by(|a, b| a.name.cmp(&b.name));
    volumes
}
