//! Opens local items with the desktop's default application.

use std::fs;
use std::process::{Command, Stdio};

use application::ports::Launcher;
use domain::{StoragePath, VfsError};

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
        self.blocking(move || {
            fs::symlink_metadata(&native).map_err(|e| from_io(&e))?;
            let mut child = Command::new(OPENER)
                .arg(&native)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| from_io(&e))?;
            // Some xdg-open handlers only return when the opened app exits, so
            // the child is reaped on its own thread instead of blocking the job.
            std::thread::spawn(move || child.wait());
            Ok(())
        })
        .await
    }
}
