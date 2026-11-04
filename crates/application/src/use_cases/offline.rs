//! Keeping remote items available offline (pinning). The cache adapter never
//! evicts pinned copies and serves pinned folders' last listing when the
//! provider can't be reached.

use std::sync::Arc;

use domain::{EntryKind, StoragePath};

use crate::operations::progress::ProgressTracker;
use crate::operations::scan::scan;
use crate::operations::{JobContext, OperationError};
use crate::ports::{AccountMounts, CacheJanitor, MountResolver, PinStore, StoreError, is_pinned};

/// Pins an item and downloads it (and, for a folder, everything in it).
pub struct MakeAvailableOffline {
    mounts: Arc<dyn MountResolver>,
    pins: Arc<dyn PinStore>,
}

impl MakeAvailableOffline {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, pins: Arc<dyn PinStore>) -> Self {
        Self { mounts, pins }
    }

    /// The pin is recorded first, so an interrupted download is completed
    /// the next time the item is opened rather than forgotten.
    ///
    /// # Errors
    /// The first item that couldn't be downloaded, or `Cancelled`.
    pub async fn execute(&self, location: &StoragePath, ctx: &JobContext) -> Result<(), OperationError> {
        let on_item = OperationError::on(location);
        let handle = self.mounts.resolve(location.mount()).map_err(OperationError::on(location))?;
        let materializer = handle.materializer().map_err(OperationError::on(location))?;
        let lister = handle.lister().map_err(OperationError::on(location))?;
        self.pins.pin(location.clone()).await.map_err(|e| OperationError::Store(e.to_string()))?;
        let root = lister.stat_boxed(location).await.map_err(on_item)?;
        let tree = scan(&*lister, root, &ctx.cancel).await.map_err(OperationError::on(location))?;
        let tracker = ProgressTracker::new(Arc::clone(&ctx.observer));
        tracker.begin(tree.item_count(), tree.bytes);
        let report = tracker.byte_callback();
        for item in &tree.items {
            if ctx.cancel.is_cancelled() {
                return Err(OperationError::Cancelled);
            }
            tracker.item_started(Some(item.name()));
            if item.kind() == EntryKind::Directory {
                // Listing the folder stores the snapshot used offline.
                let _ = futures::StreamExt::collect::<Vec<_>>(lister.list_boxed(item.path(), ctx.cancel.clone())).await;
            } else {
                materializer.local_file_boxed(item.path()).await.map_err(OperationError::on(item.path()))?;
                report(item.metadata().size.unwrap_or_default());
            }
            tracker.item_finished();
        }
        Ok(())
    }
}

/// Unpins an item; its copies become ordinary cache entries.
pub struct RemoveOfflineCopy {
    pins: Arc<dyn PinStore>,
}

impl RemoveOfflineCopy {
    #[must_use]
    pub fn new(pins: Arc<dyn PinStore>) -> Self {
        Self { pins }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, location: StoragePath) -> Result<(), StoreError> {
        self.pins.unpin(location).await
    }
}

/// Whether an item is kept offline, and through which pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfflineStatus {
    NotPinned,
    Pinned,
    /// Inside a pinned folder; only that folder can be unpinned.
    InPinnedFolder(StoragePath),
}

pub struct GetOfflineStatus {
    pins: Arc<dyn PinStore>,
}

impl GetOfflineStatus {
    #[must_use]
    pub fn new(pins: Arc<dyn PinStore>) -> Self {
        Self { pins }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, location: &StoragePath) -> Result<OfflineStatus, StoreError> {
        let pins = self.pins.pins().await?;
        if pins.contains(location) {
            return Ok(OfflineStatus::Pinned);
        }
        Ok(match pins.into_iter().find(|pin| is_pinned(std::slice::from_ref(pin), location)) {
            Some(folder) => OfflineStatus::InPinnedFolder(folder),
            None => OfflineStatus::NotPinned,
        })
    }
}

/// Removes an account's local traces after the account itself is removed:
/// nothing it ever downloaded or listed stays on this computer.
pub struct ForgetLocalCopies {
    janitor: Arc<dyn CacheJanitor>,
}

impl ForgetLocalCopies {
    #[must_use]
    pub fn new(janitor: Arc<dyn CacheJanitor>) -> Self {
        Self { janitor }
    }

    /// # Errors
    /// The cache index can't be updated; leftovers are evicted later.
    pub async fn execute(&self, account: AccountMounts) -> Result<(), StoreError> {
        self.janitor.forget(account).await
    }
}

macro_rules! opaque_debug {
    ($($name:ident),+) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )+};
}

opaque_debug!(MakeAvailableOffline, RemoveOfflineCopy, GetOfflineStatus, ForgetLocalCopies);
