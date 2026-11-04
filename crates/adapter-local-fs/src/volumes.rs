//! Mounted volumes for the sidebar: `/Volumes` on macOS; `/media/$USER`,
//! `/run/media/$USER`, `/mnt` and gvfs network mounts on Linux. On macOS also
//! the cloud folders the system syncs: iCloud Drive and every File Provider
//! folder (Google Drive, OneDrive, Dropbox, Box…) in `~/Library/CloudStorage`.

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
        let task = self.runtime.spawn_blocking(|| {
            let mut volumes: Vec<Volume> = mount_roots().iter().flat_map(|root| volumes_in(root)).collect();
            volumes.extend(cloud_folders());
            volumes
        });
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
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    vec![
        PathBuf::from("/media").join(&user),
        PathBuf::from("/run/media").join(&user),
        PathBuf::from("/mnt"),
        // Shares mounted by Nautilus or Dolphin's gvfs backend (smb, sftp, dav).
        PathBuf::from(format!("/run/user/{uid}/gvfs")),
    ]
}

#[cfg(target_os = "macos")]
const ICLOUD_DRIVE: &str = "Library/Mobile Documents/com~apple~CloudDocs";
#[cfg(target_os = "macos")]
const ICLOUD_LABEL: &str = "iCloud Drive";
#[cfg(target_os = "macos")]
const CLOUD_STORAGE: &str = "Library/CloudStorage";
/// File Provider folder prefixes and how their apps name themselves.
#[cfg(target_os = "macos")]
const CLOUD_PROVIDERS: &[(&str, &str)] = &[
    ("GoogleDrive", "Google Drive"),
    ("OneDrive", "OneDrive"),
    ("Dropbox", "Dropbox"),
    ("Box", "Box"),
    ("pCloud", "pCloud"),
];

/// iCloud Drive, then each File Provider folder (`GoogleDrive-me@x.com`).
#[cfg(target_os = "macos")]
fn cloud_folders() -> Vec<Volume> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let mut folders = Vec::new();
    let icloud = home.join(ICLOUD_DRIVE);
    if icloud.is_dir()
        && let Ok(location) = storage_path_from_native(&icloud)
    {
        folders.push(Volume { name: ICLOUD_LABEL.to_owned(), location });
    }
    let mut providers = volumes_in(&home.join(CLOUD_STORAGE));
    for provider in &mut providers {
        provider.name = provider_label(&provider.name);
    }
    folders.extend(providers);
    folders
}

#[cfg(not(target_os = "macos"))]
fn cloud_folders() -> Vec<Volume> {
    Vec::new()
}

/// `GoogleDrive-me@example.com` → `Google Drive (me@example.com)`.
#[cfg(target_os = "macos")]
fn provider_label(folder: &str) -> String {
    let (prefix, account) = folder.split_once('-').unwrap_or((folder, ""));
    let name = CLOUD_PROVIDERS.iter().find(|(known, _)| *known == prefix).map_or(prefix, |(_, label)| *label);
    if account.is_empty() || account == prefix { name.to_owned() } else { format!("{name} ({account})") }
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::provider_label;

    #[test]
    fn file_provider_folders_get_their_app_names() {
        assert_eq!(provider_label("GoogleDrive-me@example.com"), "Google Drive (me@example.com)");
        assert_eq!(provider_label("OneDrive-Personal"), "OneDrive (Personal)");
        assert_eq!(provider_label("Dropbox"), "Dropbox");
        assert_eq!(provider_label("Box-Box"), "Box");
        assert_eq!(provider_label("Acme-Work"), "Acme (Work)");
    }
}
