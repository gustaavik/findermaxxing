//! Behaviour specific to the local filesystem, beyond the shared contract.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::time::{Duration, Instant};

use adapter_local_fs::{LocalFs, storage_path_from_native};
use application::CancelToken;
use application::ports::Lister;
use domain::{EntryKind, FileEntry, SymlinkTarget, VfsError};
use futures::{StreamExt, TryStreamExt};

const LARGE_DIR_ENTRIES: usize = 100_000;
const FIRST_BATCH_BUDGET: Duration = Duration::from_millis(50);
const FULL_LISTING_BUDGET: Duration = Duration::from_secs(2);

fn local_fs() -> LocalFs {
    LocalFs::new(tokio::runtime::Handle::current())
}

async fn list(dir: &std::path::Path) -> Result<Vec<FileEntry>, VfsError> {
    let path = storage_path_from_native(dir).unwrap();
    let batches: Vec<Vec<FileEntry>> = local_fs().list(&path, CancelToken::never()).try_collect().await?;
    Ok(batches.into_iter().flatten().collect())
}

#[tokio::test(flavor = "multi_thread")]
async fn dotfiles_are_marked_hidden() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), b"").unwrap();
    std::fs::write(dir.path().join("visible"), b"").unwrap();
    let entries = list(dir.path()).await.unwrap();
    let hidden: Vec<_> = entries.iter().filter(|e| e.is_hidden()).map(|e| e.name().to_string()).collect();
    assert_eq!(hidden, [".env"]);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn symlinks_report_their_target_kind() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("to-dir")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("dangling")).unwrap();
    let mut entries = list(dir.path()).await.unwrap();
    entries.sort_by(|a, b| a.name().cmp(b.name()));
    let kinds: Vec<_> = entries.iter().map(|e| (e.name().to_string(), e.kind())).collect();
    assert_eq!(
        kinds,
        [
            ("dangling".to_owned(), EntryKind::Symlink(SymlinkTarget::Broken)),
            ("real".to_owned(), EntryKind::Directory),
            ("to-dir".to_owned(), EntryKind::Symlink(SymlinkTarget::Directory)),
        ]
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn unreadable_dir_is_permission_denied() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = list(&locked).await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Root bypasses permission bits (common in CI containers).
    if result.is_ok() {
        return;
    }
    assert_eq!(result, Err(VfsError::PermissionDenied));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "creates 100k files; run with --ignored large_dir"]
async fn large_dir_streams_quickly() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..LARGE_DIR_ENTRIES {
        std::fs::write(dir.path().join(format!("file-{i:06}.txt")), b"").unwrap();
    }
    let path = storage_path_from_native(dir.path()).unwrap();

    let start = Instant::now();
    let mut batches = Box::pin(local_fs().list(&path, CancelToken::never()));
    let first = batches.next().await.unwrap().unwrap();
    let first_batch_latency = start.elapsed();
    let rest: Vec<Vec<FileEntry>> = batches.try_collect().await.unwrap();
    let total_latency = start.elapsed();

    let total = first.len() + rest.iter().map(Vec::len).sum::<usize>();
    println!("first batch {first_batch_latency:?}, all {total} entries {total_latency:?}");
    assert_eq!(total, LARGE_DIR_ENTRIES);
    assert!(first_batch_latency < FIRST_BATCH_BUDGET, "first batch took {first_batch_latency:?}");
    assert!(total_latency < FULL_LISTING_BUDGET, "full listing took {total_latency:?}");
}
