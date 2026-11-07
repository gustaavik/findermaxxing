//! Checksums of a file on any provider, in one pass for all algorithms, and
//! the digest its publisher listed next to it, if there is one.

use std::sync::Arc;

use domain::{ByteSize, Digest, EntryKind, HashAlgorithm, StoragePath, VfsError, digest_in_list};
use futures::{StreamExt, TryStreamExt};

use crate::cancel::CancelToken;
use crate::ports::{ContentHasher, MountResolver, ProgressCallback};

/// Checksum lists bigger than this aren't checksum lists.
const MAX_LIST_SIZE: u64 = 1024 * 1024;
/// File names (lowercase) of lists that cover a whole folder.
const FOLDER_LISTS: &[&str] =
    &["sha256sums", "sha256sums.txt", "sha1sums", "md5sums", "checksums", "checksums.txt", "b3sums", "sha256sum.txt"];
/// Suffixes of a list for one file (`app.dmg.sha256`).
const FILE_LIST_SUFFIXES: &[&str] = &[".sha256", ".sha256sum", ".sha1", ".md5", ".b3", ".checksum"];

pub struct ComputeChecksums {
    mounts: Arc<dyn MountResolver>,
    hasher: Arc<dyn ContentHasher>,
}

impl ComputeChecksums {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, hasher: Arc<dyn ContentHasher>) -> Self {
        Self { mounts, hasher }
    }

    /// Digests in the order of `algorithms`; `progress` reports bytes read.
    ///
    /// # Errors
    /// Reading failed, or `cancel` fired.
    pub async fn execute(
        &self,
        file: &StoragePath,
        algorithms: &[HashAlgorithm],
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Vec<Digest>, VfsError> {
        let reader = self.mounts.resolve(file.mount())?.reader()?;
        let mut chunks = reader.read_boxed(file, cancel.clone()).await?;
        let mut running: Vec<_> = algorithms.iter().map(|a| self.hasher.start(*a)).collect();
        while let Some(chunk) = chunks.try_next().await? {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            for hash in &mut running {
                hash.update(&chunk);
            }
            progress(ByteSize(chunk.len() as u64));
        }
        Ok(running.into_iter().map(|hash| hash.finish()).collect())
    }
}

/// Looks next to a file for a checksum list that mentions it (`SHA256SUMS`,
/// `name.sha256`, …) and returns the digest listed there.
pub struct FindPublishedChecksum {
    mounts: Arc<dyn MountResolver>,
}

impl FindPublishedChecksum {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// `None` when no list mentions the file.
    ///
    /// # Errors
    /// The folder couldn't be listed.
    pub async fn execute(&self, file: &StoragePath) -> Result<Option<String>, VfsError> {
        let (Some(folder), Some(name)) = (file.parent(), file.name()) else { return Ok(None) };
        let file_name = name.display_name().into_owned();
        let mount = self.mounts.resolve(file.mount())?;
        let siblings: Vec<_> =
            mount.lister()?.list_boxed(&folder, CancelToken::never()).try_collect::<Vec<_>>().await?.concat();
        let own_list =
            |lower: &str| FILE_LIST_SUFFIXES.iter().any(|s| lower == format!("{}{s}", file_name.to_lowercase()));
        let lists: Vec<_> = siblings
            .into_iter()
            .filter_map(|entry| {
                let lower = entry.name().display_name().to_lowercase();
                let own = own_list(&lower);
                let small = entry.metadata().size.is_some_and(|s| s.0 <= MAX_LIST_SIZE);
                (entry.kind() == EntryKind::File && small && (own || FOLDER_LISTS.contains(&lower.as_str())))
                    .then_some((entry, own))
            })
            .collect();
        let reader = mount.reader()?;
        for (list, own) in lists {
            let bytes: Vec<Vec<u8>> = reader.read_boxed(list.path(), CancelToken::never()).await?.try_collect().await?;
            let text = String::from_utf8_lossy(&bytes.concat()).into_owned();
            if let Some(digest) = digest_in_list(&text, &file_name) {
                return Ok(Some(digest));
            }
            // `name.sha256` often holds just the digest.
            if own && let Some(digest) = text.split_whitespace().next() {
                return Ok(Some(digest.to_owned()));
            }
        }
        Ok(None)
    }
}

/// Hashes many files with one algorithm; used by the duplicate finder.
pub(crate) async fn hash_prefix(
    mounts: &dyn MountResolver,
    hasher: &dyn ContentHasher,
    file: &StoragePath,
    limit: Option<u64>,
    progress: &ProgressCallback,
    cancel: &CancelToken,
) -> Result<Digest, VfsError> {
    let reader = mounts.resolve(file.mount())?.reader()?;
    let mut chunks = reader.read_boxed(file, cancel.clone()).await?;
    let mut hash = hasher.start(HashAlgorithm::Blake3);
    let mut remaining = limit.unwrap_or(u64::MAX);
    while remaining > 0 {
        let Some(chunk) = chunks.next().await else { break };
        let chunk = chunk?;
        let take = usize::try_from(remaining).unwrap_or(usize::MAX).min(chunk.len());
        hash.update(&chunk[..take]);
        remaining -= take as u64;
        progress(ByteSize(take as u64));
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
    }
    Ok(hash.finish())
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
pub(crate) use opaque_debug;

opaque_debug!(ComputeChecksums, FindPublishedChecksum);
