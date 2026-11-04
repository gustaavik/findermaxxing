//! Opens local items with the desktop's default application.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use application::ports::{Launcher, LocalFile, LocalFileOpener};
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

fn launch_native(native: &Path) -> Result<(), VfsError> {
    fs::symlink_metadata(native).map_err(|e| from_io(&e))?;
    let mut child = Command::new(OPENER)
        .arg(native)
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
