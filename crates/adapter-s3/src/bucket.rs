//! One bucket as a mount. S3 has no real folders: a folder exists if any key
//! starts with its prefix or a zero-byte `prefix/` marker object exists.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use application::CancelToken;
use application::ports::{
    ByteStream, Copier, FolderMaker, ItemProperties, ItemVersion, Lister, LocalFile, Materializer, MetadataEditor,
    Mover, Presigner, ProgressCallback, PropertyChanges, Reader, Remover, VersionLister, WriteSink, Writer,
};
use aws_sdk_s3::Client;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::{ByteStream as SdkStream, DateTime};
use aws_sdk_s3::types::{Delete, MetadataDirective, ObjectIdentifier};
use domain::{
    BucketName, ByteSize, Capability, EntryBatch, EntryKind, EntryMetadata, FileEntry, LIST_BATCH_SIZE, PathError,
    StoragePath, Timestamp, VfsError, Visibility,
};
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use futures::{FutureExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::runtime::Handle;

use crate::errors::from_sdk;
use crate::keys::{DELIMITER, child_of, copy_source, key_of, prefix_of};
use crate::writer::S3Sink;

/// CopyObject's single-request limit; above it the application streams instead.
const SERVER_SIDE_COPY_LIMIT: u64 = 5 * 1024 * 1024 * 1024;
/// DeleteObjects accepts at most this many keys per request.
const DELETE_BATCH: usize = 1000;
const HIDDEN_PREFIX: char = '.';
const MILLIS_PER_SECOND: i64 = 1000;
const PARTIAL_DOWNLOAD_SUFFIX: &str = ".findermaxxing-partial";

/// One bucket as a mount; obtained from [`crate::S3Service::open_bucket`].
#[derive(Clone)]
pub struct BucketFs {
    client: Client,
    bucket: BucketName,
    runtime: Handle,
    cache: PathBuf,
}

impl BucketFs {
    pub(crate) fn new(client: Client, bucket: BucketName, runtime: Handle, cache: PathBuf) -> Self {
        Self { client, bucket, runtime, cache }
    }

    fn bucket(&self) -> &str {
        self.bucket.as_str()
    }

    /// The object at exactly `key`, if any.
    async fn head(&self, key: &str) -> Result<Option<aws_sdk_s3::operation::head_object::HeadObjectOutput>, VfsError> {
        match self.client.head_object().bucket(self.bucket()).key(key).send().await.map_err(from_sdk) {
            Ok(output) => Ok(Some(output)),
            Err(VfsError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Whether any key (or a folder marker) lives under `prefix`.
    async fn prefix_exists(&self, prefix: &str) -> Result<bool, VfsError> {
        if prefix.is_empty() {
            return Ok(true);
        }
        let output = self
            .client
            .list_objects_v2()
            .bucket(self.bucket())
            .prefix(prefix)
            .max_keys(1)
            .send()
            .await
            .map_err(from_sdk)?;
        Ok(!output.contents().is_empty() || !output.common_prefixes().is_empty())
    }

    async fn kind_of(&self, path: &StoragePath) -> Result<EntryKind, VfsError> {
        if self.head(&key_of(path)?).await?.is_some() {
            Ok(EntryKind::File)
        } else if self.prefix_exists(&prefix_of(path)?).await? {
            Ok(EntryKind::Directory)
        } else {
            Err(VfsError::NotFound)
        }
    }

    async fn require_absent(&self, path: &StoragePath) -> Result<(), VfsError> {
        match self.kind_of(path).await {
            Ok(_) => Err(VfsError::AlreadyExists),
            Err(VfsError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn require_parent(&self, path: &StoragePath) -> Result<(), VfsError> {
        match path.parent() {
            Some(parent) if !parent.is_root() => match self.kind_of(&parent).await? {
                EntryKind::Directory => Ok(()),
                _ => Err(VfsError::NotADirectory),
            },
            _ => Ok(()),
        }
    }

    /// Every key under `prefix`, recursively (no delimiter).
    async fn all_keys(&self, prefix: &str) -> Result<Vec<String>, VfsError> {
        let mut keys = Vec::new();
        let mut token = None;
        loop {
            let output = self
                .client
                .list_objects_v2()
                .bucket(self.bucket())
                .prefix(prefix)
                .set_continuation_token(token)
                .send()
                .await
                .map_err(from_sdk)?;
            keys.extend(output.contents().iter().filter_map(|object| object.key().map(str::to_owned)));
            token = output.next_continuation_token().map(str::to_owned);
            if token.is_none() {
                return Ok(keys);
            }
        }
    }

    async fn delete_keys(&self, keys: Vec<String>) -> Result<(), VfsError> {
        for batch in keys.chunks(DELETE_BATCH) {
            let objects: Result<Vec<ObjectIdentifier>, _> =
                batch.iter().map(|key| ObjectIdentifier::builder().key(key).build()).collect();
            let delete = Delete::builder()
                .set_objects(Some(objects.map_err(|e| VfsError::Io { message: e.to_string() })?))
                .quiet(true)
                .build()
                .map_err(|e| VfsError::Io { message: e.to_string() })?;
            self.client.delete_objects().bucket(self.bucket()).delete(delete).send().await.map_err(from_sdk)?;
        }
        Ok(())
    }

    async fn put_marker(&self, prefix: &str) -> Result<(), VfsError> {
        let request = self.client.put_object().bucket(self.bucket()).key(prefix).body(SdkStream::from_static(b""));
        request.send().await.map_err(from_sdk).map(drop)
    }

    /// Servers that keep real directories behind the S3 API (SeaweedFS)
    /// leave empty ones listed after their objects are gone; on AWS these
    /// deletes are cheap no-ops. One at a time, deepest first.
    async fn delete_markers(&self, markers: Vec<String>) -> Result<(), VfsError> {
        for marker in markers {
            match self.client.delete_object().bucket(self.bucket()).key(&marker).send().await.map_err(from_sdk) {
                Ok(_) | Err(VfsError::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn copy_key(&self, from: &str, to: &str) -> Result<(), VfsError> {
        self.client
            .copy_object()
            .bucket(self.bucket())
            .copy_source(copy_source(self.bucket(), from))
            .key(to)
            .send()
            .await
            .map_err(from_sdk)
            .map(drop)
    }

    async fn page(
        &self,
        dir: &StoragePath,
        prefix: &str,
        token: Option<String>,
    ) -> Result<(EntryBatch, Option<String>), VfsError> {
        let output = self
            .client
            .list_objects_v2()
            .bucket(self.bucket())
            .prefix(prefix)
            .delimiter(DELIMITER.to_string())
            .set_continuation_token(token)
            .send()
            .await
            .map_err(from_sdk)?;
        let folders = output
            .common_prefixes()
            .iter()
            .filter_map(|p| child_of(dir, prefix, p.prefix()?))
            .filter_map(|path| entry(&path, EntryKind::Directory, None, None));
        let files = output.contents().iter().filter_map(|object| {
            let key = object.key()?;
            if key.ends_with(DELIMITER) {
                return None;
            }
            let path = child_of(dir, prefix, key)?;
            let size = object.size().and_then(|s| u64::try_from(s).ok()).map(ByteSize);
            entry(&path, EntryKind::File, size, object.last_modified())
        });
        let entries = folders.chain(files).collect();
        Ok((entries, output.next_continuation_token().map(str::to_owned)))
    }

    /// An empty first page means the folder doesn't exist, unless it has a
    /// marker (an empty folder) or the path is in fact a file.
    async fn check_empty_listing(&self, dir: &StoragePath, prefix: &str) -> Result<(), VfsError> {
        if dir.is_root() || self.head(prefix).await?.is_some() {
            Ok(())
        } else if self.head(&key_of(dir)?).await?.is_some() {
            Err(VfsError::NotADirectory)
        } else {
            Err(VfsError::NotFound)
        }
    }
}

/// `None` only for the bucket root, which has no entry.
fn entry(
    path: &StoragePath,
    kind: EntryKind,
    size: Option<ByteSize>,
    modified: Option<&DateTime>,
) -> Option<FileEntry> {
    let name = path.name()?;
    let hidden = name.as_str().is_some_and(|s| s.starts_with(HIDDEN_PREFIX));
    let metadata = EntryMetadata {
        size,
        // Whole seconds: listings report milliseconds but HEAD's Last-Modified
        // header only seconds, and both must describe the item identically.
        modified: modified.map(|m| Timestamp::from_unix_millis(m.secs() * MILLIS_PER_SECOND)),
        visibility: if hidden { Visibility::Hidden } else { Visibility::Visible },
        permissions: None,
    };
    Some(FileEntry::new(&path.parent()?, name.clone(), kind, metadata))
}

struct Listing {
    fs: BucketFs,
    dir: StoragePath,
    prefix: String,
    token: Option<String>,
    first: bool,
    done: bool,
    cancel: CancelToken,
}

impl std::fmt::Debug for BucketFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BucketFs").field("bucket", &self.bucket).finish_non_exhaustive()
    }
}

impl Lister for BucketFs {
    fn list(
        &self,
        dir: &StoragePath,
        cancel: CancelToken,
    ) -> impl Stream<Item = Result<EntryBatch, VfsError>> + Send + 'static {
        let state = match prefix_of(dir) {
            Ok(prefix) => Ok(Listing {
                fs: self.clone(),
                dir: dir.clone(),
                prefix,
                token: None,
                first: true,
                done: false,
                cancel,
            }),
            Err(error) => Err(error),
        };
        let pages = stream::unfold(state.map(Some).unwrap_or_else(|_| None), |state| async move {
            let mut listing = state?;
            if listing.done {
                return None;
            }
            if listing.cancel.is_cancelled() {
                listing.done = true;
                return Some((Err(VfsError::Cancelled), Some(listing)));
            }
            let page = listing.fs.page(&listing.dir, &listing.prefix, listing.token.take()).await;
            let result = match page {
                Ok((entries, next)) => {
                    let empty_first = listing.first && entries.is_empty() && next.is_none();
                    listing.first = false;
                    listing.done = next.is_none();
                    listing.token = next;
                    if empty_first {
                        listing.fs.check_empty_listing(&listing.dir, &listing.prefix).await.map(|()| entries)
                    } else {
                        Ok(entries)
                    }
                }
                Err(error) => Err(error),
            };
            if result.is_err() {
                listing.done = true;
            }
            Some((result, Some(listing)))
        });
        pages.flat_map(|page| {
            let batches: Vec<Result<EntryBatch, VfsError>> = match page {
                Ok(entries) => entries.chunks(LIST_BATCH_SIZE).map(|chunk| Ok(chunk.to_vec())).collect(),
                Err(error) => vec![Err(error)],
            };
            stream::iter(batches)
        })
    }

    async fn stat(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        if path.is_root() {
            return Err(VfsError::InvalidPath(PathError::Root));
        }
        let key = key_of(path)?;
        if let Some(head) = self.head(&key).await? {
            let size = head.content_length().and_then(|s| u64::try_from(s).ok()).map(ByteSize);
            return entry(path, EntryKind::File, size, head.last_modified()).ok_or(VfsError::NotFound);
        }
        if self.prefix_exists(&prefix_of(path)?).await? {
            return entry(path, EntryKind::Directory, None, None).ok_or(VfsError::NotFound);
        }
        Err(VfsError::NotFound)
    }
}

impl Copier for BucketFs {
    async fn copy_item(
        &self,
        from: &StoragePath,
        to: &StoragePath,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<ByteSize, VfsError> {
        let key = key_of(from)?;
        let Some(head) = self.head(&key).await? else {
            return Err(if self.prefix_exists(&prefix_of(from)?).await? {
                VfsError::IsADirectory
            } else {
                VfsError::NotFound
            });
        };
        let size = ByteSize(head.content_length().and_then(|s| u64::try_from(s).ok()).unwrap_or_default());
        if size.0 > SERVER_SIDE_COPY_LIMIT {
            return Err(VfsError::Unsupported(Capability::ServerSideCopy));
        }
        self.require_absent(to).await?;
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        self.copy_key(&key, &key_of(to)?).await?;
        progress(size);
        Ok(size)
    }
}

impl Mover for BucketFs {
    /// Copy + delete: S3 has no rename. Not atomic (the provider doesn't
    /// declare `AtomicRename`); a failure part-way leaves both copies.
    async fn rename(&self, from: &StoragePath, to: &StoragePath) -> Result<(), VfsError> {
        let kind = self.kind_of(from).await?;
        self.require_absent(to).await?;
        if kind == EntryKind::File {
            let key = key_of(from)?;
            self.copy_key(&key, &key_of(to)?).await?;
            return self.delete_keys(vec![key]).await;
        }
        let (old_prefix, new_prefix) = (prefix_of(from)?, prefix_of(to)?);
        let keys = self.all_keys(&old_prefix).await?;
        for key in &keys {
            let target = format!("{new_prefix}{}", key.strip_prefix(&old_prefix).unwrap_or(key));
            if key.ends_with(DELIMITER) {
                // Folder markers hold no data; some servers can't copy them.
                self.put_marker(&target).await?;
            } else {
                self.copy_key(key, &target).await?;
            }
        }
        let markers = folder_markers(&old_prefix, &keys);
        self.delete_keys(keys).await?;
        self.delete_markers(markers).await
    }
}

impl Remover for BucketFs {
    async fn remove(&self, path: &StoragePath, cancel: CancelToken) -> Result<(), VfsError> {
        if cancel.is_cancelled() {
            return Err(VfsError::Cancelled);
        }
        match self.kind_of(path).await? {
            EntryKind::File => self.delete_keys(vec![key_of(path)?]).await,
            _ => {
                let prefix = prefix_of(path)?;
                let keys = self.all_keys(&prefix).await?;
                let markers = folder_markers(&prefix, &keys);
                self.delete_keys(keys).await?;
                self.delete_markers(markers).await
            }
        }
    }
}

/// Every folder prefix at or below `prefix` implied by `keys`, deepest first.
fn folder_markers(prefix: &str, keys: &[String]) -> Vec<String> {
    let mut markers: std::collections::BTreeSet<String> = std::collections::BTreeSet::from([prefix.to_owned()]);
    for key in keys {
        let mut end = key.len();
        while let Some(slash) = key[..end].rfind(DELIMITER) {
            let marker = &key[..=slash];
            if marker.len() < prefix.len() {
                break;
            }
            markers.insert(marker.to_owned());
            end = slash;
        }
    }
    let mut ordered: Vec<String> = markers.into_iter().collect();
    ordered.sort_by_key(|marker| std::cmp::Reverse(marker.len()));
    ordered
}

impl FolderMaker for BucketFs {
    async fn create_folder(&self, path: &StoragePath) -> Result<FileEntry, VfsError> {
        self.require_absent(path).await?;
        self.require_parent(path).await?;
        self.put_marker(&prefix_of(path)?).await?;
        entry(path, EntryKind::Directory, None, None).ok_or(VfsError::InvalidPath(PathError::Root))
    }
}

impl Reader for BucketFs {
    async fn read(&self, path: &StoragePath, cancel: CancelToken) -> Result<ByteStream, VfsError> {
        let key = key_of(path)?;
        let output = match self.client.get_object().bucket(self.bucket()).key(&key).send().await.map_err(from_sdk) {
            Ok(output) => output,
            Err(VfsError::NotFound) if self.prefix_exists(&prefix_of(path)?).await? => {
                return Err(VfsError::IsADirectory);
            }
            Err(error) => return Err(error),
        };
        let chunks = stream::unfold(Some(output.body), move |body| {
            let cancel = cancel.clone();
            async move {
                let mut body = body?;
                if cancel.is_cancelled() {
                    return Some((Err(VfsError::Cancelled), None));
                }
                match body.try_next().await {
                    Ok(Some(bytes)) => Some((Ok(bytes.to_vec()), Some(body))),
                    Ok(None) => None,
                    Err(error) => Some((Err(VfsError::Io { message: error.to_string() }), None)),
                }
            }
        });
        Ok(chunks.boxed())
    }
}

impl Writer for BucketFs {
    async fn create(&self, path: &StoragePath, size_hint: Option<ByteSize>) -> Result<Box<dyn WriteSink>, VfsError> {
        self.require_absent(path).await?;
        self.require_parent(path).await?;
        Ok(Box::new(S3Sink::new(
            self.client.clone(),
            self.bucket.clone(),
            key_of(path)?,
            size_hint,
            self.runtime.clone(),
        )))
    }
}

impl Materializer for BucketFs {
    /// Downloads into the cache (reused while size and date match the object).
    async fn local_file(&self, item: &StoragePath) -> Result<LocalFile, VfsError> {
        let key = key_of(item)?;
        let head = self.head(&key).await?.ok_or(VfsError::NotFound)?;
        let target = self.cache.join(&key);
        let expected = head.content_length().and_then(|s| u64::try_from(s).ok());
        if is_fresh(&target, expected).await {
            return Ok(LocalFile::new(target));
        }
        let mut chunks = self.read(item, CancelToken::never()).await?;
        download(&target, &mut chunks).await?;
        Ok(LocalFile::new(target))
    }
}

async fn is_fresh(target: &Path, expected_size: Option<u64>) -> bool {
    tokio::fs::metadata(target).await.is_ok_and(|metadata| Some(metadata.len()) == expected_size)
}

async fn download(target: &Path, chunks: &mut ByteStream) -> Result<(), VfsError> {
    let io = |e: std::io::Error| VfsError::Io { message: e.to_string() };
    if let Some(folder) = target.parent() {
        tokio::fs::create_dir_all(folder).await.map_err(io)?;
    }
    let partial = target.with_extension(PARTIAL_DOWNLOAD_SUFFIX.trim_start_matches('.'));
    let mut file = tokio::fs::File::create(&partial).await.map_err(io)?;
    while let Some(chunk) = chunks.next().await {
        file.write_all(&chunk?).await.map_err(io)?;
    }
    file.flush().await.map_err(io)?;
    tokio::fs::rename(&partial, target).await.map_err(io)
}

impl MetadataEditor for BucketFs {
    fn properties<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<ItemProperties, VfsError>> {
        async move {
            let head = self.head(&key_of(path)?).await?.ok_or(VfsError::NotFound)?;
            let mut facts = Vec::new();
            let mut fact = |label: &str, value: Option<String>| {
                if let Some(value) = value {
                    facts.push((label.to_owned(), value));
                }
            };
            fact("ETag", head.e_tag().map(str::to_owned));
            fact("Storage class", head.storage_class().map(|c| c.as_str().to_owned()));
            fact("Version", head.version_id().map(str::to_owned));
            fact("Encryption", head.server_side_encryption().map(|e| e.as_str().to_owned()));
            fact("Cache-Control", head.cache_control().map(str::to_owned));
            let user_metadata: BTreeMap<String, String> =
                head.metadata().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
            Ok(ItemProperties { content_type: head.content_type().map(str::to_owned), user_metadata, facts })
        }
        .boxed()
    }

    /// S3 metadata is immutable; "editing" copies the object onto itself
    /// with `MetadataDirective=REPLACE`.
    fn update<'a>(
        &'a self,
        path: &'a StoragePath,
        changes: &'a PropertyChanges,
    ) -> BoxFuture<'a, Result<(), VfsError>> {
        async move {
            let key = key_of(path)?;
            self.client
                .copy_object()
                .bucket(self.bucket())
                .copy_source(copy_source(self.bucket(), &key))
                .key(&key)
                .metadata_directive(MetadataDirective::Replace)
                .set_content_type(changes.content_type.clone())
                .set_metadata(Some(changes.user_metadata.clone().into_iter().collect()))
                .send()
                .await
                .map_err(from_sdk)
                .map(drop)
        }
        .boxed()
    }
}

impl Presigner for BucketFs {
    fn share_link<'a>(&'a self, path: &'a StoragePath, valid_for: Duration) -> BoxFuture<'a, Result<String, VfsError>> {
        async move {
            let config =
                PresigningConfig::expires_in(valid_for).map_err(|e| VfsError::Io { message: e.to_string() })?;
            let request = self
                .client
                .get_object()
                .bucket(self.bucket())
                .key(key_of(path)?)
                .presigned(config)
                .await
                .map_err(from_sdk)?;
            Ok(request.uri().to_owned())
        }
        .boxed()
    }
}

impl VersionLister for BucketFs {
    fn versions<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<Vec<ItemVersion>, VfsError>> {
        async move {
            let key = key_of(path)?;
            let output =
                self.client.list_object_versions().bucket(self.bucket()).prefix(&key).send().await.map_err(from_sdk)?;
            let timestamp = |m: Option<&DateTime>| m.and_then(|m| m.to_millis().ok()).map(Timestamp::from_unix_millis);
            let mut versions: Vec<ItemVersion> = output
                .versions()
                .iter()
                .filter(|v| v.key() == Some(key.as_str()))
                .map(|v| ItemVersion {
                    version_id: v.version_id().unwrap_or_default().to_owned(),
                    modified: timestamp(v.last_modified()),
                    size: v.size().and_then(|s| u64::try_from(s).ok()).map(ByteSize),
                    is_latest: v.is_latest().unwrap_or_default(),
                    is_delete_marker: false,
                })
                .chain(output.delete_markers().iter().filter(|m| m.key() == Some(key.as_str())).map(|m| ItemVersion {
                    version_id: m.version_id().unwrap_or_default().to_owned(),
                    modified: timestamp(m.last_modified()),
                    size: None,
                    is_latest: m.is_latest().unwrap_or_default(),
                    is_delete_marker: true,
                }))
                .collect();
            versions.sort_by_key(|v| std::cmp::Reverse(v.modified));
            Ok(versions)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::folder_markers;

    #[test]
    fn markers_cover_every_level_deepest_first() {
        let keys = vec!["t/b/leaf".to_owned(), "t/b/".to_owned(), "t/x".to_owned()];
        assert_eq!(folder_markers("t/", &keys), ["t/b/", "t/"]);
    }
}
