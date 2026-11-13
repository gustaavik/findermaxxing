//! Finds duplicate files below some folders, on any provider: same size,
//! then same first 64 KiB, then same content (BLAKE3). Optionally, images
//! that look the same. Nothing is changed; removing is the Trash use case.

use std::collections::HashMap;
use std::sync::Arc;

use domain::{
    ByteSize, DuplicateGroup, DuplicateMatch, EntryKind, FileEntry, ImageFormat, PARTIAL_HASH_BYTES,
    SIMILAR_IMAGE_DISTANCE, StoragePath, VfsError, keeper_first, same_size_groups, similar_groups, sort_groups,
    split_by,
};
use futures::TryStreamExt;

use super::checksums::{hash_prefix, opaque_debug};
use super::support::local_copy;
use crate::cancel::CancelToken;
use crate::operations::progress::ProgressTracker;
use crate::ports::{ContentHasher, ImageFingerprinter, JobObserver, MountResolver};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateSearch {
    pub folders: Vec<StoragePath>,
    pub min_size: ByteSize,
    pub include_hidden: bool,
    /// Also group images that look alike (decodes every image: slower).
    pub similar_images: bool,
    /// Only the folders' own files, not what's in their subfolders.
    pub top_level_only: bool,
}

pub struct FindDuplicates {
    mounts: Arc<dyn MountResolver>,
    hasher: Arc<dyn ContentHasher>,
    fingerprinter: Arc<dyn ImageFingerprinter>,
}

impl FindDuplicates {
    #[must_use]
    pub fn new(
        mounts: Arc<dyn MountResolver>,
        hasher: Arc<dyn ContentHasher>,
        fingerprinter: Arc<dyn ImageFingerprinter>,
    ) -> Self {
        Self { mounts, hasher, fingerprinter }
    }

    /// Groups with the most reclaimable space first; in each, the suggested
    /// keeper first. Unreadable files and folders are left out.
    ///
    /// # Errors
    /// Only `Cancelled`, or a top folder that can't be listed.
    pub async fn execute(
        &self,
        search: &DuplicateSearch,
        observer: Arc<dyn JobObserver>,
        cancel: CancelToken,
    ) -> Result<Vec<DuplicateGroup>, VfsError> {
        let tracker = ProgressTracker::new(observer);
        let files = self.collect_files(search, &cancel).await?;
        let candidates = same_size_groups(files.clone(), search.min_size);
        let to_read = candidates.iter().flatten().map(|f| f.metadata().size.unwrap_or_default().0).sum();
        let images: Vec<FileEntry> = if search.similar_images {
            files.into_iter().filter(|f| is_image(f) && f.metadata().size >= Some(search.min_size)).collect()
        } else {
            Vec::new()
        };
        tracker.begin((candidates.iter().map(Vec::len).sum::<usize>() + images.len()) as u64, ByteSize(to_read));
        let mut groups = Vec::new();
        for group in candidates {
            groups.extend(self.identical_within(group, &tracker, &cancel).await?);
        }
        let mut groups: Vec<DuplicateGroup> =
            groups.into_iter().map(|items| keeper_first(DuplicateMatch::Identical, items)).collect();
        if !images.is_empty() {
            let similar = self.similar_images(images, &groups, &tracker, &cancel).await?;
            groups.extend(similar.into_iter().map(|items| keeper_first(DuplicateMatch::Similar, items)));
        }
        sort_groups(&mut groups);
        Ok(groups)
    }

    async fn collect_files(&self, search: &DuplicateSearch, cancel: &CancelToken) -> Result<Vec<FileEntry>, VfsError> {
        let mut files = Vec::new();
        for folder in &search.folders {
            let lister = self.mounts.resolve(folder.mount())?.lister()?;
            let mut pending = vec![folder.clone()];
            let mut first = true;
            while let Some(dir) = pending.pop() {
                if cancel.is_cancelled() {
                    return Err(VfsError::Cancelled);
                }
                let listed = lister.list_boxed(&dir, cancel.clone()).try_collect::<Vec<_>>().await;
                let entries = match listed {
                    Ok(batches) => batches.concat(),
                    Err(VfsError::Cancelled) => return Err(VfsError::Cancelled),
                    // The folder the user picked must be readable; below it, skip what isn't.
                    Err(error) if first => return Err(error),
                    Err(_) => Vec::new(),
                };
                first = false;
                for entry in entries {
                    if entry.is_hidden() && !search.include_hidden {
                        continue;
                    }
                    match entry.kind() {
                        EntryKind::Directory if !search.top_level_only => pending.push(entry.path().clone()),
                        EntryKind::File => files.push(entry),
                        _ => {}
                    }
                }
            }
        }
        // The same folder picked twice (or nested) mustn't make files their own duplicates.
        files.sort_by_cached_key(|file| file.path().encode());
        files.dedup_by(|a, b| a.path() == b.path());
        Ok(files)
    }

    /// Splits a same-size group by the hash of the first bytes, then (for
    /// bigger files) of everything.
    async fn identical_within(
        &self,
        group: Vec<FileEntry>,
        tracker: &Arc<ProgressTracker>,
        cancel: &CancelToken,
    ) -> Result<Vec<Vec<FileEntry>>, VfsError> {
        let size = group.first().and_then(|f| f.metadata().size).unwrap_or_default();
        let partial = self.hashed(group, Some(PARTIAL_HASH_BYTES), tracker, cancel).await?;
        if size.0 <= PARTIAL_HASH_BYTES {
            return Ok(partial);
        }
        let mut identical = Vec::new();
        for same_start in partial {
            identical.extend(self.hashed(same_start, None, tracker, cancel).await?);
        }
        Ok(identical)
    }

    async fn hashed(
        &self,
        group: Vec<FileEntry>,
        limit: Option<u64>,
        tracker: &Arc<ProgressTracker>,
        cancel: &CancelToken,
    ) -> Result<Vec<Vec<FileEntry>>, VfsError> {
        let progress = tracker.byte_callback();
        let mut keyed = Vec::with_capacity(group.len());
        for file in group {
            tracker.item_started(Some(file.name()));
            match hash_prefix(&*self.mounts, &*self.hasher, file.path(), limit, &progress, cancel).await {
                Ok(digest) => keyed.push((digest, file)),
                Err(VfsError::Cancelled) => return Err(VfsError::Cancelled),
                Err(_) => {}
            }
            if limit.is_none() {
                tracker.item_finished();
            }
        }
        Ok(split_by(keyed))
    }

    async fn similar_images(
        &self,
        images: Vec<FileEntry>,
        identical: &[DuplicateGroup],
        tracker: &ProgressTracker,
        cancel: &CancelToken,
    ) -> Result<Vec<Vec<FileEntry>>, VfsError> {
        let mut fingerprinted = Vec::new();
        for image in images {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            tracker.item_started(Some(image.name()));
            if let Ok(local) = local_copy(&*self.mounts, image.path()).await
                && let Some(fingerprint) = self.fingerprinter.fingerprint(local).await
            {
                fingerprinted.push((fingerprint, image));
            }
            tracker.item_finished();
        }
        let group_of: HashMap<&StoragePath, usize> = identical
            .iter()
            .enumerate()
            .flat_map(|(index, group)| group.items.iter().map(move |item| (item.path(), index)))
            .collect();
        Ok(similar_groups(fingerprinted, SIMILAR_IMAGE_DISTANCE)
            .into_iter()
            // Groups whose images are all byte-identical copies are shown once, as identical.
            .filter(|group| {
                let first = group_of.get(group[0].path());
                first.is_none() || !group.iter().all(|item| group_of.get(item.path()) == first)
            })
            .collect())
    }
}

fn is_image(file: &FileEntry) -> bool {
    ImageFormat::of(file.name()).is_some()
}

opaque_debug!(FindDuplicates);
