//! Search, previews, saved searches and bulk rename against the in-memory fake.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use application::operations::{JobContext, OperationHistory};
use application::ports::{MountResolver, NewSavedSearch, PreviewTheme};
use application::testing::{FakeVfs, InMemorySavedSearches, RecordingObserver, StubArchiveInspector, StubTextRenderer};
use application::use_cases::{
    BulkRenameEntries, DeleteSavedSearch, ListSavedSearches, PreparePreview, PreviewBody, SaveSearch, SearchEvent,
    SearchFiles, UNTITLED_SEARCH, UndoOperation,
};
use application::{CancelToken, CancellationSource, ProviderRegistry};
use domain::{
    BulkRename, CaseSensitivity, FileCategory, FileEntry, Matching, NamePart, SearchQuery, StoragePath, Timestamp,
    VfsError,
};
use futures::StreamExt;
use futures_executor::block_on;

const NOW: Timestamp = Timestamp::from_unix_millis(0);

fn registry(fake: &FakeVfs) -> Arc<dyn MountResolver> {
    Arc::new(ProviderRegistry::default().with_provider(fake.clone().into_factory()).unwrap())
}

fn search(fake: &FakeVfs, scope: &str, text: &str) -> Vec<SearchEvent> {
    let query = SearchQuery { text: text.to_owned(), ..SearchQuery::default() };
    let events = SearchFiles::new(registry(fake)).execute(&fake.path(scope), &query, NOW, CancelToken::never());
    block_on(events.collect())
}

fn matched_names(events: &[SearchEvent]) -> Vec<String> {
    let mut names: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            SearchEvent::Matches(found) => Some(found.iter().map(|f| f.name().to_string()).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    names.sort();
    names
}

#[test]
fn search_walks_subfolders_and_skips_hidden_ones() {
    let fake = FakeVfs::new();
    fake.add_dir("/p");
    fake.add_file("/p/report-2024.pdf", 1);
    fake.add_dir("/p/archive");
    fake.add_file("/p/archive/old report.txt", 1);
    fake.add_dir("/p/.cache");
    fake.add_file("/p/.cache/report.tmp", 1);
    fake.add_file("/p/notes.md", 1);

    let events = search(&fake, "/p", "REPORT");

    assert_eq!(matched_names(&events), ["old report.txt", "report-2024.pdf"]);
    assert_eq!(events.last(), Some(&SearchEvent::Finished { folders_searched: 2, unreadable_folders: 0 }));
}

#[test]
fn search_of_a_missing_scope_fails() {
    let fake = FakeVfs::new();
    assert_eq!(search(&fake, "/nope", "x"), vec![SearchEvent::Failed(VfsError::NotFound)]);
}

#[test]
fn cancelled_search_ends_quietly() {
    let fake = FakeVfs::new();
    fake.add_dir("/p");
    let source = CancellationSource::new();
    source.cancel();
    let events: Vec<SearchEvent> = block_on(
        SearchFiles::new(registry(&fake))
            .execute(&fake.path("/p"), &SearchQuery::default(), NOW, source.token())
            .collect(),
    );
    assert!(events.is_empty());
}

fn preview(fake: &FakeVfs, path: &str) -> PreviewBody {
    let use_case = PreparePreview::new(registry(fake), Arc::new(StubTextRenderer), Arc::new(StubArchiveInspector));
    block_on(use_case.execute(&fake.path(path), PreviewTheme::Light)).unwrap().body
}

#[test]
fn previews_are_routed_by_category() {
    let fake = FakeVfs::new();
    fake.add_dir("/d");
    for name in ["a.png", "b.pdf", "c.mp4", "d.flac", "e.rs", "f.zip", "g.docx", "h.bin", "README"] {
        fake.add_file(&format!("/d/{name}"), 1);
    }
    let routed: Vec<(&str, &str)> = ["a.png", "b.pdf", "c.mp4", "d.flac", "e.rs", "f.zip", "g.docx", "h.bin", "README"]
        .iter()
        .map(|name| (*name, body_kind(&preview(&fake, &format!("/d/{name}")))))
        .collect();
    assert_eq!(
        routed,
        [
            ("a.png", "image"),
            ("b.pdf", "pdf"),
            ("c.mp4", "video"),
            ("d.flac", "audio"),
            ("e.rs", "text"),
            ("f.zip", "archive"),
            ("g.docx", "summary"),
            ("h.bin", "summary"),
            ("README", "text"),
        ]
    );
    assert_eq!(preview(&fake, "/d"), PreviewBody::Folder { items: 9 });
}

fn body_kind(body: &PreviewBody) -> &'static str {
    match body {
        PreviewBody::Image(_) => "image",
        PreviewBody::Pdf(_) => "pdf",
        PreviewBody::Video(_) => "video",
        PreviewBody::Audio(_) => "audio",
        PreviewBody::Text(_) => "text",
        PreviewBody::Archive(_) => "archive",
        PreviewBody::Folder { .. } => "folder",
        PreviewBody::Summary { .. } => "summary",
    }
}

#[test]
fn saved_searches_round_trip() {
    let store = Arc::new(InMemorySavedSearches::default());
    let scope = StoragePath::parse("file:///Users/me").unwrap();
    let query = SearchQuery { text: "invoice".into(), category: Some(FileCategory::Pdf), ..SearchQuery::default() };
    let saved =
        block_on(SaveSearch::new(store.clone()).execute(NewSavedSearch { name: "  ".into(), scope, query })).unwrap();
    assert_eq!(saved.name, UNTITLED_SEARCH);

    let listed = block_on(ListSavedSearches::new(store.clone()).execute()).unwrap();
    assert_eq!(listed, vec![saved.clone()]);
    block_on(DeleteSavedSearch::new(store.clone()).execute(saved.id)).unwrap();
    assert!(block_on(ListSavedSearches::new(store).execute()).unwrap().is_empty());
}

fn entries(fake: &FakeVfs, paths: &[&str]) -> Vec<FileEntry> {
    paths
        .iter()
        .map(|p| match fake.upserted(p) {
            domain::ChangeEvent::Upserted(entry) => entry,
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn bulk_rename_swaps_names_and_undoes_as_one_step() {
    let fake = FakeVfs::new();
    fake.add_dir("/d");
    fake.add_file_with("/d/a", b"was a");
    fake.add_file_with("/d/b", b"was b");
    let mounts = registry(&fake);
    let history = Arc::new(OperationHistory::default());
    // a → b and b → a in one batch.
    let rule = BulkRename::replace("^(a|b)$", "x", Matching::Pattern, CaseSensitivity::Sensitive, NamePart::WholeName)
        .unwrap();
    let mut plan = rule.plan(&entries(&fake, &["/d/a", "/d/b"]), |_| false);
    plan[0].outcome = domain::RenameOutcome::Rename(domain::PathSegment::new("b").unwrap());
    plan[1].outcome = domain::RenameOutcome::Rename(domain::PathSegment::new("a").unwrap());
    let observer = Arc::new(RecordingObserver::default());
    let ctx = JobContext::unattended(observer, CancelToken::never());

    block_on(BulkRenameEntries::new(mounts.clone(), history.clone()).execute(&plan, &ctx)).result.unwrap();
    assert_eq!(fake.contents("/d/a").unwrap(), b"was b");
    assert_eq!(fake.contents("/d/b").unwrap(), b"was a");

    block_on(UndoOperation::new(mounts, history).execute(&ctx)).unwrap().result.unwrap();
    assert_eq!(fake.contents("/d/a").unwrap(), b"was a");
    assert_eq!(fake.contents("/d/b").unwrap(), b"was b");
}

#[test]
fn bulk_rename_skips_problem_items() {
    let fake = FakeVfs::new();
    fake.add_dir("/d");
    fake.add_file("/d/one.txt", 1);
    fake.add_file("/d/two.txt", 1);
    fake.add_file("/d/taken.txt", 1);
    let rule = BulkRename::format("taken", 1, 1);
    let existing = |p: &StoragePath| fake.exists(&p.path_string());
    let plan = rule.plan(&entries(&fake, &["/d/one.txt", "/d/two.txt"]), existing);
    let ctx = JobContext::unattended(Arc::new(RecordingObserver::default()), CancelToken::never());
    block_on(BulkRenameEntries::new(registry(&fake), Arc::default()).execute(&plan, &ctx)).result.unwrap();
    assert!(fake.exists("/d/taken 1.txt") && fake.exists("/d/taken 2.txt"));
    assert!(fake.exists("/d/taken.txt"));
}
