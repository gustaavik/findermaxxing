//! Contract cases for the mutating roles. Shared rule: nothing ever
//! overwrites, and failed or cancelled work leaves no partial result.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use application::ports::{Copier, FolderMaker, Lister, Mover, ProgressCallback, Remover, Trasher};
use application::{CancelToken, CancellationSource};
use domain::{ByteSize, EntryKind, VfsError};

use crate::{VfsFixture, child, list_all};

/// Expands to the copy/move/remove/create-folder contract cases.
#[macro_export]
macro_rules! mutation_contract_tests {
    ($make_fixture:expr) => {
        $crate::vfs_contract_tests!(@cases $make_fixture;
            copy_item_copies_bytes_and_reports_progress,
            copy_item_never_overwrites,
            copy_item_rejects_directories,
            copy_item_missing_source_is_not_found,
            cancelled_copy_leaves_nothing_behind,
            rename_moves_the_item,
            rename_never_overwrites,
            rename_can_change_only_letter_case,
            rename_missing_source_is_not_found,
            remove_deletes_whole_trees,
            remove_missing_is_not_found,
            create_folder_creates_once,
            create_folder_needs_a_parent,
        );
    };
}

/// Expands to the trash contract cases. These use the real platform trash
/// for local adapters and restore everything they trash.
#[macro_export]
macro_rules! trash_contract_tests {
    ($make_fixture:expr) => {
        $crate::vfs_contract_tests!(@cases $make_fixture;
            trash_then_restore_round_trips,
            restore_onto_an_occupied_place_is_already_exists,
            restoring_twice_is_not_in_trash,
            trashing_missing_is_not_found,
        );
    };
}

fn counting_progress() -> (ProgressCallback, Arc<AtomicU64>) {
    let total = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&total);
    (
        Arc::new(move |bytes: ByteSize| {
            counter.fetch_add(bytes.0, Ordering::SeqCst);
        }),
        total,
    )
}

fn no_progress() -> ProgressCallback {
    Arc::new(|_| {})
}

pub async fn copy_item_copies_bytes_and_reports_progress<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Copier,
{
    fixture.create_file("source.bin", b"0123456789").await;
    let (progress, reported) = counting_progress();
    let copied = fixture
        .vfs()
        .copy_item(&child(fixture, "source.bin"), &child(fixture, "copy.bin"), progress, CancelToken::never())
        .await;
    assert_eq!(copied, Ok(ByteSize(10)));
    assert_eq!(reported.load(Ordering::SeqCst), 10, "progress must add up to the bytes copied");
    assert_eq!(fixture.read_file("copy.bin").await.as_deref(), Some(b"0123456789".as_slice()));
    assert_eq!(fixture.read_file("source.bin").await.as_deref(), Some(b"0123456789".as_slice()));
}

pub async fn copy_item_never_overwrites<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Copier,
{
    fixture.create_file("a", b"new").await;
    fixture.create_file("b", b"old").await;
    let result =
        fixture.vfs().copy_item(&child(fixture, "a"), &child(fixture, "b"), no_progress(), CancelToken::never()).await;
    assert_eq!(result, Err(VfsError::AlreadyExists));
    assert_eq!(fixture.read_file("b").await.as_deref(), Some(b"old".as_slice()));
}

pub async fn copy_item_rejects_directories<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Copier,
{
    fixture.create_dir("dir").await;
    let result = fixture
        .vfs()
        .copy_item(&child(fixture, "dir"), &child(fixture, "x"), no_progress(), CancelToken::never())
        .await;
    assert_eq!(result, Err(VfsError::IsADirectory));
}

pub async fn copy_item_missing_source_is_not_found<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Copier,
{
    let result = fixture
        .vfs()
        .copy_item(&child(fixture, "nope"), &child(fixture, "x"), no_progress(), CancelToken::never())
        .await;
    assert_eq!(result, Err(VfsError::NotFound));
}

pub async fn cancelled_copy_leaves_nothing_behind<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Copier,
{
    fixture.create_file("big", &vec![7_u8; 1 << 20]).await;
    let source = CancellationSource::new();
    source.cancel();
    let result =
        fixture.vfs().copy_item(&child(fixture, "big"), &child(fixture, "copy"), no_progress(), source.token()).await;
    assert_eq!(result, Err(VfsError::Cancelled));
    assert_eq!(fixture.read_file("copy").await, None);
}

pub async fn rename_moves_the_item<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Mover,
{
    fixture.create_dir("into").await;
    fixture.create_file("a", b"payload").await;
    fixture.vfs().rename(&child(fixture, "a"), &child(fixture, "into").join(segment("b"))).await.unwrap();
    assert_eq!(fixture.read_file("a").await, None);
    assert_eq!(fixture.read_file("into/b").await.as_deref(), Some(b"payload".as_slice()));
}

pub async fn rename_never_overwrites<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Mover,
{
    fixture.create_file("a", b"a").await;
    fixture.create_file("b", b"b").await;
    assert_eq!(fixture.vfs().rename(&child(fixture, "a"), &child(fixture, "b")).await, Err(VfsError::AlreadyExists));
    assert_eq!(fixture.read_file("a").await.as_deref(), Some(b"a".as_slice()));
    assert_eq!(fixture.read_file("b").await.as_deref(), Some(b"b".as_slice()));
}

/// On case-insensitive filesystems (macOS default) `photo.JPG` → `photo.jpg`
/// targets the same item; that must still work.
pub async fn rename_can_change_only_letter_case<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Mover + Lister,
{
    fixture.create_file("photo.JPG", b"img").await;
    fixture.vfs().rename(&child(fixture, "photo.JPG"), &child(fixture, "photo.jpg")).await.unwrap();
    let names: Vec<String> =
        list_all(fixture, &fixture.root()).await.unwrap().iter().map(|e| e.name().to_string()).collect();
    assert_eq!(names, ["photo.jpg"]);
}

pub async fn rename_missing_source_is_not_found<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Mover,
{
    assert_eq!(fixture.vfs().rename(&child(fixture, "nope"), &child(fixture, "x")).await, Err(VfsError::NotFound));
}

pub async fn remove_deletes_whole_trees<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Remover + Lister,
{
    fixture.create_dir("tree").await;
    fixture.create_dir("tree/branch").await;
    fixture.create_file("tree/branch/leaf", b"x").await;
    fixture.vfs().remove(&child(fixture, "tree"), CancelToken::never()).await.unwrap();
    assert_eq!(list_all(fixture, &fixture.root()).await, Ok(vec![]));
}

pub async fn remove_missing_is_not_found<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Remover,
{
    assert_eq!(fixture.vfs().remove(&child(fixture, "nope"), CancelToken::never()).await, Err(VfsError::NotFound));
}

pub async fn create_folder_creates_once<F: VfsFixture>(fixture: &F)
where
    F::Vfs: FolderMaker,
{
    let created = fixture.vfs().create_folder(&child(fixture, "new")).await.unwrap();
    assert_eq!(created.kind(), EntryKind::Directory);
    assert_eq!(created.path(), &child(fixture, "new"));
    assert_eq!(fixture.vfs().create_folder(&child(fixture, "new")).await, Err(VfsError::AlreadyExists));
}

pub async fn create_folder_needs_a_parent<F: VfsFixture>(fixture: &F)
where
    F::Vfs: FolderMaker,
{
    let orphan = child(fixture, "missing").join(segment("new"));
    assert_eq!(fixture.vfs().create_folder(&orphan).await, Err(VfsError::NotFound));
}

pub async fn trash_then_restore_round_trips<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Trasher,
{
    fixture.create_dir("folder").await;
    fixture.create_file("folder/inside", b"precious").await;
    let ticket = fixture.vfs().trash(&child(fixture, "folder")).await.unwrap();
    assert_eq!(ticket.original, child(fixture, "folder"));
    assert_ne!(ticket.trashed, ticket.original);
    assert_eq!(fixture.read_file("folder/inside").await, None);

    fixture.vfs().restore(&ticket).await.unwrap();
    assert_eq!(fixture.read_file("folder/inside").await.as_deref(), Some(b"precious".as_slice()));
}

pub async fn restore_onto_an_occupied_place_is_already_exists<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Trasher,
{
    fixture.create_file("a", b"first").await;
    let ticket = fixture.vfs().trash(&child(fixture, "a")).await.unwrap();
    fixture.create_file("a", b"second").await;
    assert_eq!(fixture.vfs().restore(&ticket).await, Err(VfsError::AlreadyExists));
    assert_eq!(fixture.read_file("a").await.as_deref(), Some(b"second".as_slice()));
    // Leave the real trash as we found it.
    fixture.remove_file("a").await;
    fixture.vfs().restore(&ticket).await.unwrap();
}

pub async fn restoring_twice_is_not_in_trash<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Trasher,
{
    fixture.create_file("a", b"x").await;
    let ticket = fixture.vfs().trash(&child(fixture, "a")).await.unwrap();
    fixture.vfs().restore(&ticket).await.unwrap();
    fixture.remove_file("a").await;
    assert_eq!(fixture.vfs().restore(&ticket).await, Err(VfsError::NotInTrash));
}

pub async fn trashing_missing_is_not_found<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Trasher,
{
    assert_eq!(fixture.vfs().trash(&child(fixture, "nope")).await.map(drop), Err(VfsError::NotFound));
}

fn segment(name: &str) -> domain::PathSegment {
    domain::PathSegment::new(name).unwrap()
}
