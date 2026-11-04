//! Opens local items with the desktop's default application, and web pages
//! (OAuth sign-in) in the default browser.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use application::ports::{Launcher, LocalFile, LocalFileOpener, UrlOpener};
use domain::{StoragePath, VfsError};
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::runtime::Handle;

use crate::error::from_io;
use crate::lister::LocalFs;
use crate::native_path::to_native;

#[cfg(target_os = "macos")]
const OPENER: &str = "open";
#[cfg(not(target_os = "macos"))]
const OPENER: &str = "xdg-open";

impl Launcher for LocalFs {
    async fn launch(&self, path: &StoragePath) -> Result<(), VfsError> {
        let native = to_native(path)?;
        self.blocking(move || launch_native(&native)).await
    }
}

/// Opens materialised copies of remote items.
#[derive(Debug, Clone)]
pub struct LocalOpener {
    runtime: Handle,
}

impl LocalOpener {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }
}

impl LocalFileOpener for LocalOpener {
    fn open(&self, file: &LocalFile) -> BoxFuture<'static, Result<(), VfsError>> {
        let path = file.path().to_path_buf();
        let task = self.runtime.spawn_blocking(move || launch_native(&path));
        async move { task.await.map_err(|join| VfsError::Io { message: join.to_string() })? }.boxed()
    }
}

const WEB_SCHEMES: [&str; 2] = ["https://", "http://"];

impl UrlOpener for LocalOpener {
    /// Only web pages: a URL is handed to `open`/`xdg-open`, which would
    /// also run local files or other handlers if given anything else.
    fn open_url(&self, url: &str) -> BoxFuture<'static, Result<(), VfsError>> {
        let url = url.to_owned();
        let task = self.runtime.spawn_blocking(move || {
            if !WEB_SCHEMES.iter().any(|scheme| url.starts_with(scheme)) {
                return Err(VfsError::InvalidPath(domain::PathError::InvalidScheme(url)));
            }
            spawn_opener(std::ffi::OsStr::new(&url))
        });
        async move { task.await.map_err(|join| VfsError::Io { message: join.to_string() })? }.boxed()
    }
}

fn launch_native(native: &Path) -> Result<(), VfsError> {
    fs::symlink_metadata(native).map_err(|e| from_io(&e))?;
    spawn_opener(native.as_os_str())
}

fn spawn_opener(target: &std::ffi::OsStr) -> Result<(), VfsError> {
    let mut child = Command::new(OPENER)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| from_io(&e))?;
    // Some xdg-open handlers only return when the opened app exits, so the
    // child is reaped on its own thread instead of blocking the job.
    std::thread::spawn(move || child.wait());
    Ok(())
}
