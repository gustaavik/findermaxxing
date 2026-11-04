# ADR 0014 — S3 provider on aws-sdk-s3, tested against SeaweedFS

**Status:** accepted (M4)

## Context
S3 has no directories, no atomic rename, no Trash and no change notifications, and every request costs money.
It must still look like a folder tree and pass the same contract suite as the local disk.

## Decision
- **`adapter-s3`** uses `aws-sdk-s3` + `aws-config`. Default features are disabled (`behavior-version-latest`,
  `default-https-client`, `http-1x`, `rt-tokio`, `sigv4a`), which drops the legacy rustls 0.21 stack that
  cargo-deny flags. Works with AWS and S3-compatible servers (path-style or virtual-hosted addressing).
- **Locations:** `s3://<account-id>@<bucket>/<key>`. The account id is the mount authority, so two accounts
  can open the same bucket. Prefixes (delimiter `/`) are folders; an empty `key/` object is a folder marker.
- **Roles:** `Lister`, `Copier` (server-side `CopyObject`), `Mover` (copy + delete), `Remover`, `FolderMaker`,
  `Reader`, `Writer`, `Materializer` (download to `~/Library/Caches/FinderMaxxing/s3`), `MetadataEditor`,
  `Presigner`, `VersionLister`. No `Watcher`, `Trasher` or `Launcher`.
  Capabilities: `Metered`, `Presign`, `ListVersions`, `EditMetadata`; *not* `AtomicRename` or `RealDirectories`.
- **No overwrite, ever:** every PUT and `CompleteMultipartUpload` sends `If-None-Match: *`, so a clash is
  `AlreadyExists` and goes through the normal conflict prompt.
- **Uploads stream:** one PUT below 8 MiB; above that, multipart with 8 MiB parts, 4 in flight. A failed or
  dropped upload is aborted so no orphaned parts stay billed.
- **Folder renames/deletes** walk the prefix, deepest first, and also remove folder markers; markers are recreated
  rather than copied. Some servers (SeaweedFS) keep real directories behind prefixes, and this leaves nothing behind.
- **Timestamps are truncated to whole seconds**, because listings and HEAD report different precision.
- **Testing:** `crates/adapter-s3/tests/s3_contract.rs` runs the lister, mutation and stream contract suites
  (29 cases) against a real server. They are `#[ignore]`d locally and run in CI against **SeaweedFS** in Docker.
  MinIO's images are no longer pullable from Docker Hub or quay.io.

## Consequences
- S3 folder views don't update by themselves; after a job the app reloads views of the folders it touched
  (`folders_changed`), and `Cmd+R` reloads on demand.
- Known limits: `CopyObject` is single-request (objects over 5 GB can't be copied server-side yet; M5 adds
  `UploadPartCopy`). Uploads don't resume after a restart. The download cache has no eviction yet.
