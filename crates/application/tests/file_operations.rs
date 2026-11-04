//! File-operation use cases against the in-memory fake: behaviour, conflicts,
//! progress, partial failure and undo/redo.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use application::operations::{JobContext, OperationError, OperationHistory};
use application::testing::{FakeVfs, RecordingObserver, ScriptedResolver};
use application::use_cases::{
    CopyEntries, CreateFolder, DeletePermanently, MoveEntries, OpenEntry, RedoOperation, RenameEntry, TransferRequest,
    TrashEntries, UndoOperation,
};
use application::{CancellationSource, ProviderRegistry};
use domain::{ConflictChoice, ConflictResolution, ConflictScope, PathSegment, VfsError};
use futures_executor::block_on;

struct World {
    fake: FakeVfs,
    history: Arc<OperationHistory>,
    registry: Arc<ProviderRegistry>,
    observer: Arc<RecordingObserver>,
}

impl World {
    fn new() -> Self {
        let fake = FakeVfs::new();
        let registry = Arc::new(ProviderRegistry::default().with_provider(fake.clone().into_factory()).unwrap());
        Self { fake, history: Arc::default(), registry, observer: Arc::default() }
    }

    fn ctx(&self, answers: Vec<ConflictResolution>) -> (JobContext, Arc<ScriptedResolver>) {
        let resolver = Arc::new(ScriptedResolver::new(answers));
        let ctx = JobContext {
            observer: self.observer.clone(),
            resolver: resolver.clone(),
            cancel: application::CancelToken::never(),
        };
        (ctx, resolver)
    }

    fn plain_ctx(&self) -> JobContext {
        self.ctx(vec![answer(ConflictChoice::Stop, ConflictScope::ThisItem)]).0
    }

    fn request(&self, sources: &[&str], destination: &str) -> TransferRequest {
        TransferRequest {
            sources: sources.iter().map(|s| self.fake.path(s)).collect(),
            destination: self.fake.path(destination),
        }
    }

    fn copy(&self) -> CopyEntries {
        CopyEntries::new(self.registry.clone(), self.history.clone())
    }

    fn mover(&self) -> MoveEntries {
        MoveEntries::new(self.registry.clone(), self.history.clone())
    }

    fn undo(&self) {
        let report =
            block_on(UndoOperation::new(self.registry.clone(), self.history.clone()).execute(&self.plain_ctx()));
        report.unwrap().result.unwrap();
    }

    fn redo(&self) {
        let report =
            block_on(RedoOperation::new(self.registry.clone(), self.history.clone()).execute(&self.plain_ctx()));
        report.unwrap().result.unwrap();
    }
}

fn answer(choice: ConflictChoice, scope: ConflictScope) -> ConflictResolution {
    ConflictResolution { choice, scope }
}

#[test]
fn copy_then_undo_then_redo() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/dst");
    w.fake.add_file_with("/src/a.txt", b"hello");

    let report = block_on(w.copy().execute(&w.request(&["/src/a.txt"], "/dst"), &w.plain_ctx()));
    report.result.unwrap();
    assert_eq!(w.fake.contents("/dst/a.txt").unwrap(), b"hello");

    w.undo();
    assert!(!w.fake.exists("/dst/a.txt"), "undo of a copy removes the copy");
    assert_eq!(w.fake.trash_len(), 1, "…by trashing it, not deleting it");
    assert!(w.fake.exists("/src/a.txt"));

    w.redo();
    assert_eq!(w.fake.contents("/dst/a.txt").unwrap(), b"hello");
}

#[test]
fn copying_a_tree_reports_full_progress() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/src/docs");
    w.fake.add_dir("/src/docs/deep");
    w.fake.add_file("/src/docs/one", 10);
    w.fake.add_file("/src/docs/deep/two", 30);
    w.fake.add_dir("/dst");

    block_on(w.copy().execute(&w.request(&["/src/docs"], "/dst"), &w.plain_ctx())).result.unwrap();

    assert_eq!(w.fake.contents("/dst/docs/deep/two").unwrap().len(), 30);
    let progress = w.observer.last().unwrap();
    assert_eq!((progress.items_done, progress.items_total), (4, 4));
    assert_eq!((progress.bytes_done.0, progress.bytes_total.0), (40, 40));
    assert_eq!(progress.fraction(), Some(1.0));
}

#[test]
fn copying_into_the_same_folder_duplicates() {
    let w = World::new();
    w.fake.add_dir("/d");
    w.fake.add_file("/d/a.txt", 1);
    w.fake.add_file("/d/a 2.txt", 1);
    block_on(w.copy().execute(&w.request(&["/d/a.txt"], "/d"), &w.plain_ctx())).result.unwrap();
    assert!(w.fake.exists("/d/a 3.txt"));
}

#[test]
fn replace_trashes_the_existing_item_and_undo_brings_it_back() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/dst");
    w.fake.add_file_with("/src/a.txt", b"new");
    w.fake.add_file_with("/dst/a.txt", b"old");
    let (ctx, _) = w.ctx(vec![answer(ConflictChoice::Replace, ConflictScope::ThisItem)]);

    block_on(w.copy().execute(&w.request(&["/src/a.txt"], "/dst"), &ctx)).result.unwrap();
    assert_eq!(w.fake.contents("/dst/a.txt").unwrap(), b"new");

    w.undo();
    assert_eq!(w.fake.contents("/dst/a.txt").unwrap(), b"old");
}

#[test]
fn apply_to_all_asks_only_once() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/dst");
    for name in ["a", "b", "c"] {
        w.fake.add_file(&format!("/src/{name}"), 1);
        w.fake.add_file(&format!("/dst/{name}"), 9);
    }
    let (ctx, resolver) = w.ctx(vec![answer(ConflictChoice::Skip, ConflictScope::AllRemaining)]);

    block_on(w.copy().execute(&w.request(&["/src/a", "/src/b", "/src/c"], "/dst"), &ctx)).result.unwrap();

    assert_eq!(resolver.times_asked(), 1);
    assert_eq!(w.fake.contents("/dst/b").unwrap().len(), 9, "skipped items stay untouched");
    assert_eq!(w.observer.last().unwrap().fraction(), Some(1.0), "skipped items still complete the bar");
    assert!(w.history.next_undo().is_none(), "nothing happened, so nothing to undo");
}

#[test]
fn keep_both_uses_a_numbered_name() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/dst");
    w.fake.add_file_with("/src/a.txt", b"new");
    w.fake.add_file_with("/dst/a.txt", b"old");
    let (ctx, _) = w.ctx(vec![answer(ConflictChoice::KeepBoth, ConflictScope::ThisItem)]);
    block_on(w.copy().execute(&w.request(&["/src/a.txt"], "/dst"), &ctx)).result.unwrap();
    assert_eq!(w.fake.contents("/dst/a 2.txt").unwrap(), b"new");
    assert_eq!(w.fake.contents("/dst/a.txt").unwrap(), b"old");
}

#[test]
fn stop_ends_the_job_and_keeps_what_was_done() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/dst");
    w.fake.add_file("/src/a", 1);
    w.fake.add_file("/src/b", 1);
    w.fake.add_file("/dst/b", 1);
    let (ctx, _) = w.ctx(vec![answer(ConflictChoice::Stop, ConflictScope::ThisItem)]);
    let report = block_on(w.copy().execute(&w.request(&["/src/a", "/src/b"], "/dst"), &ctx));
    assert_eq!(report.result, Err(OperationError::Stopped));
    assert!(w.fake.exists("/dst/a"));
    w.undo();
    assert!(!w.fake.exists("/dst/a"));
}

#[test]
fn a_folder_cannot_go_into_itself() {
    let w = World::new();
    w.fake.add_dir("/a");
    w.fake.add_dir("/a/inner");
    let report = block_on(w.copy().execute(&w.request(&["/a"], "/a/inner"), &w.plain_ctx()));
    assert_eq!(report.result, Err(OperationError::IntoItself { item: w.fake.path("/a") }));
}

#[test]
fn failure_mid_job_records_the_completed_part() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/dst");
    for name in ["a", "b", "c"] {
        w.fake.add_file(&format!("/src/{name}"), 1);
    }
    w.fake.fail_on("/src/b", VfsError::PermissionDenied);

    let report = block_on(w.copy().execute(&w.request(&["/src/a", "/src/b", "/src/c"], "/dst"), &w.plain_ctx()));

    assert_eq!(
        report.result,
        Err(OperationError::Failed { error: VfsError::PermissionDenied, item: w.fake.path("/src/b") })
    );
    assert!(w.fake.exists("/dst/a") && !w.fake.exists("/dst/c"));
    w.undo();
    assert!(!w.fake.exists("/dst/a"), "the completed part is undoable");
}

#[test]
fn move_then_undo() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/src/folder");
    w.fake.add_file_with("/src/folder/f", b"x");
    w.fake.add_dir("/dst");

    block_on(w.mover().execute(&w.request(&["/src/folder"], "/dst"), &w.plain_ctx())).result.unwrap();
    assert!(!w.fake.exists("/src/folder") && w.fake.contents("/dst/folder/f").unwrap() == b"x");

    w.undo();
    assert!(w.fake.exists("/src/folder/f") && !w.fake.exists("/dst/folder"));
}

#[test]
fn cross_device_move_falls_back_to_copy_and_delete() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/src/folder");
    w.fake.add_file("/src/folder/f", 7);
    w.fake.add_dir("/dst");
    w.fake.fail_rename("/src/folder", VfsError::CrossDevice);

    block_on(w.mover().execute(&w.request(&["/src/folder"], "/dst"), &w.plain_ctx())).result.unwrap();

    assert!(!w.fake.exists("/src/folder"));
    assert_eq!(w.fake.contents("/dst/folder/f").unwrap().len(), 7);
    let progress = w.observer.last().unwrap();
    assert_eq!((progress.items_done, progress.items_total), (2, 2));
}

#[test]
fn moving_into_its_own_folder_is_a_no_op() {
    let w = World::new();
    w.fake.add_dir("/d");
    w.fake.add_file("/d/a", 1);
    block_on(w.mover().execute(&w.request(&["/d/a"], "/d"), &w.plain_ctx())).result.unwrap();
    assert!(w.fake.exists("/d/a"));
    assert!(w.history.next_undo().is_none());
}

#[test]
fn trash_and_undo_restore() {
    let w = World::new();
    w.fake.add_dir("/d");
    w.fake.add_file_with("/d/a", b"keep me");
    let trash = TrashEntries::new(w.registry.clone(), w.history.clone());
    block_on(trash.execute(&[w.fake.path("/d/a")], &w.plain_ctx())).result.unwrap();
    assert!(!w.fake.exists("/d/a"));
    w.undo();
    assert_eq!(w.fake.contents("/d/a").unwrap(), b"keep me");
}

#[test]
fn undo_that_cannot_run_keeps_its_entry() {
    let w = World::new();
    w.fake.add_dir("/d");
    w.fake.add_file("/d/a", 1);
    let trash = TrashEntries::new(w.registry.clone(), w.history.clone());
    block_on(trash.execute(&[w.fake.path("/d/a")], &w.plain_ctx())).result.unwrap();
    w.fake.add_file("/d/a", 2);

    let undo = UndoOperation::new(w.registry.clone(), w.history.clone());
    let report = block_on(undo.execute(&w.plain_ctx())).unwrap();

    assert!(matches!(report.result, Err(OperationError::Failed { error: VfsError::AlreadyExists, .. })));
    assert!(w.history.next_undo().is_some(), "the user can retry after moving the blocker away");
}

#[test]
fn rename_and_undo() {
    let w = World::new();
    w.fake.add_dir("/d");
    w.fake.add_file("/d/old", 1);
    w.fake.add_file("/d/taken", 1);
    let rename = RenameEntry::new(w.registry.clone(), w.history.clone());

    let taken = block_on(rename.execute(&w.fake.path("/d/old"), PathSegment::new("taken").unwrap()));
    assert!(matches!(taken, Err(OperationError::Failed { error: VfsError::AlreadyExists, .. })));

    let renamed = block_on(rename.execute(&w.fake.path("/d/old"), PathSegment::new("new").unwrap())).unwrap();
    assert_eq!(renamed, w.fake.path("/d/new"));
    w.undo();
    assert!(w.fake.exists("/d/old") && !w.fake.exists("/d/new"));
}

#[test]
fn create_folder_picks_a_free_name_and_undo_trashes_it() {
    let w = World::new();
    w.fake.add_dir("/d");
    let create = CreateFolder::new(w.registry.clone(), w.history.clone());
    let base = PathSegment::new("untitled folder").unwrap();
    let first = block_on(create.execute(&w.fake.path("/d"), &base)).unwrap();
    let second = block_on(create.execute(&w.fake.path("/d"), &base)).unwrap();
    assert_eq!(first, w.fake.path("/d/untitled folder"));
    assert_eq!(second, w.fake.path("/d/untitled folder 2"));
    w.undo();
    assert!(!w.fake.exists("/d/untitled folder 2"));
}

#[test]
fn permanent_delete_is_not_undoable() {
    let w = World::new();
    w.fake.add_dir("/d");
    w.fake.add_dir("/d/sub");
    w.fake.add_file("/d/sub/f", 1);
    let delete = DeletePermanently::new(w.registry.clone());
    block_on(delete.execute(&[w.fake.path("/d/sub")], &w.plain_ctx())).unwrap();
    assert!(!w.fake.exists("/d/sub") && !w.fake.exists("/d/sub/f"));
    assert_eq!(w.fake.trash_len(), 0);
    assert!(w.history.next_undo().is_none());
}

#[test]
fn open_launches_the_item() {
    let w = World::new();
    w.fake.add_file("/doc.pdf", 1);
    block_on(OpenEntry::new(w.registry.clone()).execute(&w.fake.path("/doc.pdf"))).unwrap();
    assert_eq!(w.fake.launched(), vec![w.fake.path("/doc.pdf")]);
}

#[test]
fn cancelled_job_reports_cancelled() {
    let w = World::new();
    w.fake.add_dir("/src");
    w.fake.add_dir("/dst");
    w.fake.add_file("/src/a", 1);
    let source = CancellationSource::new();
    source.cancel();
    let ctx = JobContext { cancel: source.token(), ..w.plain_ctx() };
    let report = block_on(w.copy().execute(&w.request(&["/src/a"], "/dst"), &ctx));
    assert_eq!(report.result, Err(OperationError::Cancelled));
    assert!(!w.fake.exists("/dst/a"));
}
