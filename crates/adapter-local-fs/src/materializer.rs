//! Local items already are local files.

use std::fs;

use application::ports::{LocalFile, Materializer};
use domain::{StoragePath, VfsError};

use crate::error::from_io;
use crate::lister::LocalFs;
use crate::native_path::to_native;

impl Materializer for LocalFs {
    async fn local_file(&self, item: &StoragePath) -> Result<LocalFile, VfsError> {
        let native = to_native(item)?;
        self.blocking(move || {
            fs::symlink_metadata(&native).map_err(|e| from_io(&e))?;
            Ok(LocalFile::new(native))
        })
        .await
    }
}
