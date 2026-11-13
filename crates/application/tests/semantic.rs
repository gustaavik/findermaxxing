//! The indexer and semantic search against fakes: a bag-of-words "model",
//! an in-memory vector index, and `FakeVfs` files.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use application::operations::OperationHistory;
use application::ports::{
    AiSettings, AiSettingsStore, EmbedError, Embedding, ModelId, ModelPack, MountResolver, SemanticIndex, StoreError,
    TextEmbedder,
};
use application::testing::{
    FakeExtractor, FakeImageEmbedder, FakeModelInstaller, FakeTextEmbedder, FakeVfs, InMemoryAiSettings,
    InMemorySemanticIndex, InstantClock, RecordingIndexObserver,
};
use application::use_cases::{
    AiSearchServices, ChangeIndexScope, DisableAiSearch, EnableAiSearch, Indexer, IndexerControl, IndexerParts,
    PassOutcome, PauseIndexing, ScopeChange, SemanticSearch, SemanticSearchError, indexer,
};
use application::{CancelToken, ProviderRegistry};
use domain::{IndexStatus, QueryClock, StoragePath, Timestamp};
use futures::FutureExt;
use futures::future::BoxFuture;
use futures_executor::block_on;

/// 2026-11-04 12:00 UTC.
const NOW: Timestamp = Timestamp::from_unix_millis(1_793_793_600_000);
const CLOCK: QueryClock = QueryClock { now: NOW, utc_offset_minutes: 0 };
const DAY: i64 = 86_400_000;

struct World {
    fake: FakeVfs,
    text: Arc<FakeTextEmbedder>,
    index: Arc<InMemorySemanticIndex>,
    settings: Arc<InMemoryAiSettings>,
    installer: Arc<FakeModelInstaller>,
    observer: Arc<RecordingIndexObserver>,
    indexer: Indexer,
    control: IndexerControl,
    services: AiSearchServices,
    search: SemanticSearch,
    _history: Arc<OperationHistory>,
}

fn world() -> World {
    build(false)
}

/// With `yielding`, embedding and settings reads give way to other tasks
/// halfway, so concurrent work really interleaves.
fn build(yielding: bool) -> World {
    let fake = FakeVfs::new();
    for folder in ["/home", "/home/Documents", "/home/Documents/node_modules", "/home/Pictures", "/home/Private"] {
        fake.add_dir(folder);
    }
    fake.add_file_with("/home/Documents/invoice-electricity.txt", b"Invoice for electricity, amount due 1200 DKK");
    fake.add_file_with("/home/Documents/lasagne.md", b"Recipe: lasagne with tomato and basil");
    fake.add_file_with("/home/Documents/.secret.txt", b"invoice hidden");
    fake.add_file_with("/home/Documents/node_modules/readme.md", b"invoice dependency");
    fake.add_file_with("/home/Private/diary.txt", b"invoice private diary");
    fake.add_file_with("/home/Pictures/beach sunset.jpg", b"jpeg");
    fake.add_file_with("/home/Pictures/cat sofa.jpg", b"jpeg");
    let mounts: Arc<dyn MountResolver> =
        Arc::new(ProviderRegistry::default().with_provider(fake.clone().into_factory()).unwrap());
    let (text, index) = (Arc::new(FakeTextEmbedder::default()), Arc::new(InMemorySemanticIndex::default()));
    let (settings, installer) = (Arc::new(InMemoryAiSettings::default()), Arc::new(FakeModelInstaller::default()));
    let observer = Arc::new(RecordingIndexObserver::default());
    let (text_port, settings_port): (Arc<dyn TextEmbedder>, Arc<dyn AiSettingsStore>) = if yielding {
        (Arc::new(Yielding(text.clone())), Arc::new(Yielding(settings.clone())))
    } else {
        (text.clone(), settings.clone())
    };
    let (indexer, control) = indexer(IndexerParts {
        mounts,
        extractor: Arc::new(FakeExtractor::new(fake.clone())),
        text: text_port,
        images: Arc::new(FakeImageEmbedder),
        index: index.clone(),
        settings: settings.clone(),
        installer: installer.clone(),
        clock: Arc::new(InstantClock),
        observer: observer.clone(),
    });
    let services = AiSearchServices {
        settings: settings_port,
        installer: installer.clone(),
        index: index.clone(),
        control: control.clone(),
        default_scope: vec![fake.path("/home/Documents"), fake.path("/home/Pictures")],
    };
    let search = SemanticSearch::new(
        text.clone(),
        Arc::new(FakeImageEmbedder),
        index.clone(),
        settings.clone(),
        installer.clone(),
    );
    World {
        fake,
        text,
        index,
        settings,
        installer,
        observer,
        indexer,
        control,
        services,
        search,
        _history: Arc::default(),
    }
}

fn enable(w: &World, pack: ModelPack) {
    block_on(EnableAiSearch::new(w.services.clone()).execute(pack, Arc::new(|_| {}), CancelToken::never())).unwrap();
}

/// Runs passes until one isn't interrupted by the commands the setup queued.
fn index(w: &mut World) -> PassOutcome {
    loop {
        match block_on(w.indexer.pass()) {
            PassOutcome::Interrupted => {}
            outcome => return outcome,
        }
    }
}

fn names(results: &[application::use_cases::SemanticResult]) -> Vec<String> {
    results.iter().map(|r| r.entry.name().display_name().into_owned()).collect()
}

#[test]
fn nothing_happens_until_ai_search_is_turned_on() {
    let mut w = world();
    assert_eq!(index(&mut w), PassOutcome::Idle);
    assert_eq!(w.observer.last(), Some(IndexStatus::Off));
    assert_eq!(block_on(w.search.execute("invoice", CLOCK, Vec::new())), Err(SemanticSearchError::Off));
}

#[test]
fn indexes_the_scope_but_not_hidden_excluded_or_dependency_files() {
    let mut w = world();
    enable(&w, ModelPack::Text);
    assert!(w.installer_installed(ModelPack::Text));
    assert_eq!(index(&mut w), PassOutcome::Complete { indexed: 2 });
    assert_eq!(w.observer.last(), Some(IndexStatus::UpToDate { indexed: 2 }));
    let indexed = block_on(w.index.locations_within(&w.fake.path("/home"))).unwrap();
    let mut indexed: Vec<String> = indexed.iter().map(StoragePath::path_string).collect();
    indexed.sort();
    assert_eq!(indexed, ["/home/Documents/invoice-electricity.txt", "/home/Documents/lasagne.md"]);
}

#[test]
fn unchanged_files_are_not_read_again_and_changes_are_picked_up() {
    let mut w = world();
    enable(&w, ModelPack::Text);
    index(&mut w);
    let first = w.text.passages_embedded();
    index(&mut w);
    assert_eq!(w.text.passages_embedded(), first, "an unchanged pass embeds nothing");

    w.fake.remove("/home/Documents/lasagne.md");
    w.fake.add_file_with("/home/Documents/notes.txt", b"Meeting notes about the electricity contract");
    w.fake.set_modified("/home/Documents/invoice-electricity.txt", Timestamp::from_unix_millis(5));
    index(&mut w);
    assert_eq!(w.text.passages_embedded(), first + 2, "the new and the changed file");
    let indexed = block_on(w.index.locations_within(&w.fake.path("/home"))).unwrap();
    assert!(!indexed.contains(&w.fake.path("/home/Documents/lasagne.md")), "removed files are forgotten");
}

#[test]
fn search_finds_by_meaning_and_honours_dates() {
    let mut w = world();
    enable(&w, ModelPack::Text);
    w.fake.set_modified(
        "/home/Documents/invoice-electricity.txt",
        Timestamp::from_unix_millis(NOW.unix_millis() - 20 * DAY),
    );
    w.fake.set_modified("/home/Documents/lasagne.md", Timestamp::from_unix_millis(NOW.unix_millis() - 2 * DAY));
    index(&mut w);

    let results = block_on(w.search.execute("electricity invoice", CLOCK, Vec::new())).unwrap();
    assert_eq!(names(&results)[0], "invoice-electricity.txt");
    assert!(results[0].snippet.as_deref().unwrap().starts_with("Invoice for electricity"));

    let last_month = block_on(w.search.execute("electricity invoice from last month", CLOCK, Vec::new())).unwrap();
    assert_eq!(names(&last_month), ["invoice-electricity.txt"], "modified in October");
    let this_week = block_on(w.search.execute("electricity invoice from this week", CLOCK, Vec::new())).unwrap();
    assert!(this_week.is_empty());
    let recent = block_on(w.search.execute("documents from this week", CLOCK, Vec::new())).unwrap();
    assert!(recent.is_empty() || names(&recent) == ["lasagne.md"], "filters alone list matching items: {recent:?}");
}

#[test]
fn photos_are_searched_only_once_image_search_is_on() {
    let mut w = world();
    enable(&w, ModelPack::Text);
    index(&mut w);
    let off = block_on(w.search.execute("photos of the beach", CLOCK, Vec::new())).unwrap();
    assert!(off.is_empty());

    enable(&w, ModelPack::Images);
    index(&mut w);
    let on = block_on(w.search.execute("photos of the beach", CLOCK, Vec::new())).unwrap();
    assert_eq!(names(&on)[0], "beach sunset.jpg");
    let mixed = block_on(w.search.execute("beach", CLOCK, Vec::new())).unwrap();
    assert!(names(&mixed).contains(&"beach sunset.jpg".to_owned()), "text and image results are merged");
}

#[test]
fn pausing_and_scope_changes_take_effect_on_the_next_pass() {
    let mut w = world();
    enable(&w, ModelPack::Text);
    block_on(PauseIndexing::new(w.services.clone()).execute(true)).unwrap();
    assert_eq!(index(&mut w), PassOutcome::Idle);
    assert!(matches!(w.observer.last(), Some(IndexStatus::Paused { .. })));

    block_on(PauseIndexing::new(w.services.clone()).execute(false)).unwrap();
    let private = w.fake.path("/home/Private");
    block_on(ChangeIndexScope::new(w.services.clone()).execute(ScopeChange::Include(private.clone()))).unwrap();
    assert_eq!(index(&mut w), PassOutcome::Complete { indexed: 3 });
    let within = block_on(w.search.execute("diary", CLOCK, vec![private.clone()])).unwrap();
    assert_eq!(names(&within), ["diary.txt"], "search can be limited to a folder");

    block_on(ChangeIndexScope::new(w.services.clone()).execute(ScopeChange::Forget(private))).unwrap();
    assert_eq!(index(&mut w), PassOutcome::Complete { indexed: 2 }, "a folder taken off the scope is forgotten");
    let saved = block_on(w.settings.load()).unwrap().unwrap();
    assert_eq!(saved.scope.include.len(), 2);
    w.control.rescan();
}

trait Installed {
    fn installer_installed(&self, pack: ModelPack) -> bool;
}

impl Installed for World {
    fn installer_installed(&self, pack: ModelPack) -> bool {
        use application::ports::ModelInstaller;
        self.installer.installed(pack)
    }
}

/// Pending once, then ready.
struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

struct Yielding<T>(Arc<T>);

impl<T: TextEmbedder + 'static> TextEmbedder for Yielding<T> {
    fn model(&self) -> ModelId {
        self.0.model()
    }

    fn relevance_floor(&self) -> f32 {
        self.0.relevance_floor()
    }

    fn grouping_threshold(&self) -> f32 {
        self.0.grouping_threshold()
    }

    fn embed_passages(&self, passages: Vec<String>) -> BoxFuture<'static, Result<Vec<Embedding>, EmbedError>> {
        let embedded = self.0.embed_passages(passages);
        async move {
            YieldNow(false).await;
            embedded.await
        }
        .boxed()
    }

    fn embed_query(&self, query: &str) -> BoxFuture<'static, Result<Embedding, EmbedError>> {
        self.0.embed_query(query)
    }
}

impl<T: AiSettingsStore + 'static> AiSettingsStore for Yielding<T> {
    fn load(&self) -> BoxFuture<'static, Result<Option<AiSettings>, StoreError>> {
        let loaded = self.0.load();
        async move {
            YieldNow(false).await;
            loaded.await
        }
        .boxed()
    }

    fn save(&self, settings: AiSettings) -> BoxFuture<'static, Result<(), StoreError>> {
        self.0.save(settings)
    }
}

#[test]
fn concurrent_scope_changes_are_all_kept() {
    let w = build(true);
    let (a, b) = (w.fake.path("/home/Documents"), w.fake.path("/home/Pictures"));
    let private = w.fake.path("/home/Private");
    let changes = ChangeIndexScope::new(w.services.clone());
    let results = block_on(futures::future::join_all([
        changes.execute(ScopeChange::Forget(a)),
        changes.execute(ScopeChange::Forget(b)),
        changes.execute(ScopeChange::Include(private.clone())),
    ]));
    assert!(results.iter().all(Result::is_ok));
    let saved = block_on(w.settings.load()).unwrap().unwrap();
    assert_eq!(saved.scope.include, [private]);
}

#[test]
fn deleting_the_index_wins_over_an_item_being_embedded() {
    let mut w = build(true);
    enable(&w, ModelPack::Text);
    // The pass the enable command interrupts, before anything is embedded.
    assert_eq!(block_on(w.indexer.pass()), PassOutcome::Interrupted);
    // The pass starts first and stops halfway through embedding an item;
    // the disable runs in that gap.
    let disable = DisableAiSearch::new(w.services.clone());
    let (outcome, disabled) = block_on(futures::future::join(w.indexer.pass(), disable.execute(ModelPack::Text, true)));
    disabled.unwrap();
    assert_eq!(outcome, PassOutcome::Interrupted);
    assert_eq!(block_on(w.index.count()).unwrap(), 0, "nothing written after the clear");
    assert_eq!(index(&mut w), PassOutcome::Idle);
    assert_eq!(block_on(w.index.count()).unwrap(), 0);
}
