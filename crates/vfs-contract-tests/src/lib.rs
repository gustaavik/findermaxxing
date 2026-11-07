//! Shared behavioural contract for every `Vfs` role.
//!
//! An adapter proves conformance by implementing [`VfsFixture`] and invoking
//! [`vfs_contract_tests!`] (listing + watching), [`mutation_contract_tests!`]
//! (copy, move, remove, create folder) and [`trash_contract_tests!`] for the
//! roles it offers. Each expands to one `#[tokio::test]` per contract case.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "this crate is test code")]

use std::future::Future;
use std::time::Duration;

use application::CancelToken;
use application::CancellationSource;
use application::ports::{Lister, Watcher};
use domain::{ChangeEvent, EntryKind, FileEntry, LIST_BATCH_SIZE, PathSegment, StoragePath, VfsError};
use futures::{StreamExt, TryStreamExt};

#[doc(hidden)]
pub use tokio as __tokio;

/// How long a watcher may take to report a change before the contract fails.
pub const WATCH_DEADLINE: Duration = Duration::from_secs(5);

/// A fresh, empty working directory on the provider under test.
pub trait VfsFixture: Send + Sync {
    /// The provider under test. Each contract case states the roles it needs.
    type Vfs;

    fn vfs(&self) -> &Self::Vfs;

    /// An existing, empty directory owned by this fixture.
    fn root(&self) -> StoragePath;

    fn create_dir(&self, relative: &str) -> impl Future<Output = ()> + Send;

    fn create_file(&self, relative: &str, contents: &[u8]) -> impl Future<Output = ()> + Send;

    fn remove_file(&self, relative: &str) -> impl Future<Output = ()> + Send;

    /// File contents, or `None` if nothing exists at `relative`.
    fn read_file(&self, relative: &str) -> impl Future<Output = Option<Vec<u8>>> + Send;
}

mod mutation;
mod permissions;
mod stream;
pub use mutation::*;
pub use permissions::*;
pub use stream::*;

/// Listing and watching: what every local-style provider offers.
#[macro_export]
macro_rules! vfs_contract_tests {
    ($make_fixture:expr $(, #[$attr:meta])*) => {
        $crate::lister_contract_tests!($make_fixture $(, #[$attr])*);
        $crate::watcher_contract_tests!($make_fixture $(, #[$attr])*);
    };
    // One case per step: attributes and cases can't share a repetition depth.
    (@cases $make_fixture:expr; [$($attrs:tt)*]; $case:ident $(, $rest:ident)* $(,)?) => {
        #[$crate::__tokio::test(flavor = "multi_thread")]
        $($attrs)*
        async fn $case() {
            let fixture = ($make_fixture)().await;
            $crate::$case(&fixture).await;
        }
        $crate::vfs_contract_tests!(@cases $make_fixture; [$($attrs)*]; $($rest),*);
    };
    (@cases $make_fixture:expr; [$($attrs:tt)*];) => {};
}

/// Listing cases. Extra attributes (e.g. `#[ignore = "needs MinIO"]`) apply to every case.
#[macro_export]
macro_rules! lister_contract_tests {
    ($make_fixture:expr $(, #[$attr:meta])*) => {
        $crate::vfs_contract_tests!(@cases $make_fixture; [$(#[$attr])*];
            empty_dir_lists_nothing,
            lists_children_with_kinds_and_sizes,
            missing_dir_is_not_found,
            file_is_not_a_directory,
            batches_respect_limit,
            cancellation_ends_listing,
            stat_matches_listing,
            stat_missing_is_not_found,
        );
    };
}

/// Watching cases (providers that can notify about changes).
#[macro_export]
macro_rules! watcher_contract_tests {
    ($make_fixture:expr $(, #[$attr:meta])*) => {
        $crate::vfs_contract_tests!(@cases $make_fixture; [$(#[$attr])*];
            watch_missing_dir_is_not_found,
            watcher_reports_create_and_remove,
        );
    };
}

pub(crate) fn child(fixture: &impl VfsFixture, name: &str) -> StoragePath {
    fixture.root().join(PathSegment::new(name).unwrap())
}

pub(crate) async fn list_all<F: VfsFixture>(fixture: &F, dir: &StoragePath) -> Result<Vec<FileEntry>, VfsError>
where
    F::Vfs: Lister,
{
    let batches: Vec<Vec<FileEntry>> = fixture.vfs().list(dir, CancelToken::never()).try_collect().await?;
    let mut entries: Vec<FileEntry> = batches.into_iter().flatten().collect();
    entries.sort_by(|a, b| a.name().cmp(b.name()));
    Ok(entries)
}

pub async fn empty_dir_lists_nothing<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Lister,
{
    assert_eq!(list_all(fixture, &fixture.root()).await, Ok(vec![]));
}

pub async fn lists_children_with_kinds_and_sizes<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Lister,
{
    fixture.create_dir("sub").await;
    fixture.create_file("hello.txt", b"hello").await;

    let entries = list_all(fixture, &fixture.root()).await.unwrap();

    let summary: Vec<(String, EntryKind, Option<u64>)> = entries
        .iter()
        .map(|e| {
            (e.name().to_string(), e.kind(), e.metadata().size.map(|s| s.0).filter(|_| e.kind() == EntryKind::File))
        })
        .collect();
    assert_eq!(
        summary,
        [("hello.txt".to_owned(), EntryKind::File, Some(5)), ("sub".to_owned(), EntryKind::Directory, None)]
    );
    assert!(entries.iter().all(|e| e.path().is_child_of(&fixture.root())));
}

pub async fn missing_dir_is_not_found<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Lister,
{
    assert_eq!(list_all(fixture, &child(fixture, "missing")).await, Err(VfsError::NotFound));
}

pub async fn file_is_not_a_directory<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Lister,
{
    fixture.create_file("plain", b"x").await;
    assert_eq!(list_all(fixture, &child(fixture, "plain")).await, Err(VfsError::NotADirectory));
}

pub async fn batches_respect_limit<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Lister,
{
    let total = LIST_BATCH_SIZE * 2 + 1;
    for i in 0..total {
        fixture.create_file(&format!("f{i:05}"), b"").await;
    }
    let batches: Vec<Vec<FileEntry>> =
        fixture.vfs().list(&fixture.root(), CancelToken::never()).try_collect().await.unwrap();
    assert!(batches.iter().all(|b| b.len() <= LIST_BATCH_SIZE), "a batch exceeded LIST_BATCH_SIZE");
    assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), total);
}

pub async fn cancellation_ends_listing<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Lister,
{
    for i in 0..LIST_BATCH_SIZE * 2 {
        fixture.create_file(&format!("f{i:05}"), b"").await;
    }
    let source = CancellationSource::new();
    source.cancel();
    let items: Vec<Result<Vec<FileEntry>, VfsError>> =
        fixture.vfs().list(&fixture.root(), source.token()).collect().await;
    assert_eq!(items.last(), Some(&Err(VfsError::Cancelled)), "a cancelled listing must end with Cancelled");
    assert_eq!(items.iter().filter(|i| i.is_err()).count(), 1, "nothing may follow Cancelled");
}

pub async fn stat_matches_listing<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Lister,
{
    fixture.create_file("a.bin", b"1234").await;
    let listed = list_all(fixture, &fixture.root()).await.unwrap();
    let stat = fixture.vfs().stat(&child(fixture, "a.bin")).await.unwrap();
    assert_eq!(listed, vec![stat]);
}

pub async fn stat_missing_is_not_found<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Lister,
{
    assert_eq!(fixture.vfs().stat(&child(fixture, "nope")).await, Err(VfsError::NotFound));
}

pub async fn watch_missing_dir_is_not_found<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Watcher,
{
    assert!(matches!(fixture.vfs().watch(&child(fixture, "missing")), Err(VfsError::NotFound)));
}

pub async fn watcher_reports_create_and_remove<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Watcher,
{
    let target = child(fixture, "watched.txt");
    let mut events = Box::pin(fixture.vfs().watch(&fixture.root()).unwrap());

    fixture.create_file("watched.txt", b"x").await;
    wait_for(&mut events, |e| matches!(e, ChangeEvent::Upserted(entry) if entry.path() == &target)).await;

    fixture.remove_file("watched.txt").await;
    wait_for(&mut events, |e| e == &ChangeEvent::Removed(target.clone())).await;
}

async fn wait_for<S>(events: &mut S, predicate: impl Fn(&ChangeEvent) -> bool)
where
    S: futures::Stream<Item = ChangeEvent> + Unpin,
{
    let found = tokio::time::timeout(WATCH_DEADLINE, async {
        while let Some(event) = events.next().await {
            if predicate(&event) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(found, Ok(true), "watcher did not report the expected change within {WATCH_DEADLINE:?}");
}
