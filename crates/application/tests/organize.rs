//! Organization suggestions against fakes: clutter, photos, documents, the
//! local model's groups, the cloud model (only when opted in, and only
//! summaries), and applying a plan as one undoable step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::collections::HashSet;
use std::sync::Arc;

use application::operations::{JobContext, NullObserver, OperationHistory};
use application::ports::{
    CloudModelConfig, CloudProvider, CredentialStore, ModelError, ModelInstaller, ModelPack, ModelProposal,
    MountResolver, ProposedGroup, ProposedRename, ProposedTags, Secret,
};
use application::testing::{
    FakeExtractor, FakeHasher, FakeImageTools, FakeModelInstaller, FakeTextEmbedder, FakeVfs, InMemoryCloudModels,
    InMemoryKeychain, RecordingObserver, ScriptedModel,
};
use application::use_cases::{
    AnalyzeFolder, ApplySuggestions, CloudModelSetup, CloudParts, CloudUse, EXCERPT_CHARS, FindDuplicates,
    OrganizeParts, PreviewCloudRequest, UndoOperation,
};
use application::{CancelToken, ProviderRegistry};
use domain::{Change, ClutterPolicy, ClutterReason, Plan, QueryClock, SuggestionSource, Timestamp, resolve};
use futures_executor::block_on;

/// 2026-11-04 12:00 UTC.
const NOW: Timestamp = Timestamp::from_unix_millis(1_793_793_600_000);
const CLOCK: QueryClock = QueryClock { now: NOW, utc_offset_minutes: 0 };
const DAY: i64 = 86_400_000;

struct World {
    fake: FakeVfs,
    remote: FakeVfs,
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
    installer: Arc<FakeModelInstaller>,
    cloud: CloudParts,
    model: Arc<ScriptedModel>,
    keychain: Arc<InMemoryKeychain>,
}

impl World {
    fn new(answer: Result<ModelProposal, ModelError>) -> Self {
        let fake = FakeVfs::new();
        let remote = FakeVfs::remote("s3", None);
        let registry = ProviderRegistry::default()
            .with_provider(fake.clone().into_factory())
            .unwrap()
            .with_provider(remote.clone().into_factory())
            .unwrap();
        let model = Arc::new(ScriptedModel::new(answer));
        let keychain = Arc::new(InMemoryKeychain::default());
        let cloud = CloudParts {
            store: Arc::new(InMemoryCloudModels::default()),
            credentials: keychain.clone(),
            model: model.clone(),
        };
        Self {
            fake,
            remote,
            mounts: Arc::new(registry),
            history: Arc::default(),
            installer: Arc::default(),
            cloud,
            model,
            keychain,
        }
    }

    fn analyzer(&self, images: &FakeVfs) -> AnalyzeFolder {
        let tools = Arc::new(FakeImageTools::new(images.clone()));
        AnalyzeFolder::new(OrganizeParts {
            mounts: self.mounts.clone(),
            extractor: Arc::new(FakeExtractor::new(images.clone())),
            text: Arc::new(FakeTextEmbedder::default()),
            installer: self.installer.clone(),
            image_metadata: tools.clone(),
            duplicates: Arc::new(FindDuplicates::new(self.mounts.clone(), Arc::new(FakeHasher), tools)),
            cloud: self.cloud.clone(),
            policy: ClutterPolicy::default(),
        })
    }

    fn analyze(&self, folder: &str, cloud: CloudUse) -> application::use_cases::Analysis {
        block_on(self.analyzer(&self.fake).execute(
            &self.fake.path(folder),
            CLOCK,
            cloud,
            Arc::new(NullObserver),
            CancelToken::never(),
        ))
        .unwrap()
    }

    fn install_text_model(&self) {
        block_on(self.installer.install(ModelPack::Text, Arc::new(|_| {}), CancelToken::never())).unwrap();
    }

    fn ctx(&self) -> JobContext {
        JobContext::unattended(Arc::new(RecordingObserver::default()), CancelToken::never())
    }

    fn file(&self, path: &str, content: &str, days_old: i64) {
        self.fake.add_file_with(path, content.as_bytes());
        self.fake.set_modified(path, Timestamp::from_unix_millis(NOW.unix_millis() - days_old * DAY));
    }

    fn setup(&self) -> CloudModelSetup {
        CloudModelSetup::new(self.cloud.store.clone(), self.cloud.credentials.clone())
    }
}

fn changes_for<'a>(plan: &'a Plan, name: &str) -> Vec<&'a Change> {
    plan.suggestions
        .iter()
        .filter(|s| s.item.name().is_some_and(|n| n.display_name() == name))
        .map(|s| &s.change)
        .collect()
}

#[test]
fn downloads_clutter_and_duplicates_are_suggested_for_the_trash() {
    let w = World::new(Ok(ModelProposal::default()));
    w.fake.add_dir("/Downloads");
    w.file("/Downloads/Zoom.dmg", "installer", 40);
    w.file("/Downloads/old-menu.pdf", "lunch menu", 200);
    w.file("/Downloads/report.pdf", "quarterly report", 1);
    w.file("/Downloads/report (1).pdf", "quarterly report", 1);
    let analysis = w.analyze("/Downloads", CloudUse::Never);
    eprintln!(
        "PLAN {:#?}",
        analysis.plan.suggestions.iter().map(|s| (s.item.path_string(), &s.change)).collect::<Vec<_>>()
    );
    assert!(matches!(changes_for(&analysis.plan, "Zoom.dmg")[0], Change::Trash(ClutterReason::Installer { days: 40 })));
    assert!(matches!(changes_for(&analysis.plan, "old-menu.pdf")[0], Change::Trash(ClutterReason::OldDownload { .. })));
    let duplicate = changes_for(&analysis.plan, "report (1).pdf");
    assert!(matches!(duplicate[0], Change::Trash(ClutterReason::DuplicateOf(name)) if name == "report.pdf"));
    assert!(
        changes_for(&analysis.plan, "report.pdf").iter().all(|c| !matches!(c, Change::Trash(_))),
        "the keeper stays"
    );
}

#[test]
fn photos_get_dated_names_screenshots_and_months_get_folders() {
    let w = World::new(Ok(ModelProposal::default()));
    w.fake.add_dir("/Pictures");
    w.file("/Pictures/IMG_1234.JPG", "pixels\nEXIF:DateTimeOriginal=2026:03:14 15:09:26", 10);
    for i in 0..6 {
        w.file(&format!("/Pictures/Screenshot 2026-03-0{i} at 10.00.png"), &format!("shot {i}"), 5);
    }
    for (month, days) in [("a", 60), ("b", 30)] {
        for i in 0..8 {
            w.file(&format!("/Pictures/holiday-{month}{i}.jpg"), &format!("{month}{i}"), days);
        }
    }
    let analysis = w.analyze("/Pictures", CloudUse::Never);
    let renamed = changes_for(&analysis.plan, "IMG_1234.JPG");
    assert!(renamed.iter().any(|c| matches!(c, Change::Rename(n) if n.display_name() == "2026-03-14 15.09.26.jpg")));
    let shot = changes_for(&analysis.plan, "Screenshot 2026-03-00 at 10.00.png");
    assert!(matches!(shot[0], Change::MoveInto(n) if n.display_name() == "Screenshots"));
    let month = changes_for(&analysis.plan, "holiday-a0.jpg");
    assert!(month.iter().any(|c| matches!(c, Change::MoveInto(n) if n.display_name().starts_with("Photos 2026-"))));
}

#[test]
fn documents_are_titled_tagged_and_grouped_by_the_local_model() {
    let w = World::new(Ok(ModelProposal::default()));
    w.fake.add_dir("/Docs");
    w.file("/Docs/scan0001.pdf", "Lease agreement for the flat\nParties...", 3);
    for (i, month) in ["march", "april", "may"].iter().enumerate() {
        w.file(
            &format!("/Docs/e{i}.txt"),
            &format!("electricity invoice {month} electricity company invoice total"),
            3,
        );
    }
    w.file("/Docs/lasagne.txt", "recipe lasagne tomato basil", 3);
    let without_model = w.analyze("/Docs", CloudUse::Never);
    assert!(without_model.notes.iter().any(|n| n.contains("Turn on AI search")));
    let titled = changes_for(&without_model.plan, "scan0001.pdf");
    assert!(
        titled.iter().any(|c| matches!(c, Change::Rename(n) if n.display_name() == "Lease agreement for the flat.pdf"))
    );
    let tagged = changes_for(&without_model.plan, "e0.txt");
    assert!(tagged.iter().any(|c| matches!(c, Change::AddTags(t) if t[0].as_str() == "Invoice")));

    w.install_text_model();
    let with_model = w.analyze("/Docs", CloudUse::Never);
    let grouped: Vec<_> = with_model
        .plan
        .suggestions
        .iter()
        .filter(|s| s.source == SuggestionSource::LocalModel)
        .map(|s| (s.item.name().unwrap().display_name().into_owned(), s.change.clone()))
        .collect();
    assert_eq!(grouped.len(), 3);
    assert!(grouped.iter().all(|(_, c)| matches!(c, Change::MoveInto(n) if n.display_name() == "Electricity")));
}

#[test]
fn an_applied_plan_is_one_undoable_step() {
    let w = World::new(Ok(ModelProposal::default()));
    w.fake.add_dir("/Docs");
    w.file("/Docs/scan0001.pdf", "Lease agreement for the flat", 3);
    w.file("/Docs/invoice.txt", "Invoice number 7", 3);
    w.file("/Docs/old.dmg", "installer", 60);
    let analysis = w.analyze("/Docs", CloudUse::Never);
    let accepted: HashSet<_> = analysis.plan.suggestions.iter().map(|s| s.id).collect();
    let resolution = resolve(&analysis.plan, &accepted, &analysis.existing).unwrap();
    let apply = ApplySuggestions::new(w.mounts.clone(), w.history.clone());
    block_on(apply.execute(&resolution, &w.ctx())).result.unwrap();
    assert!(w.fake.contents("/Docs/Lease agreement for the flat.pdf").is_some());
    assert!(w.fake.contents("/Docs/old.dmg").is_none());
    assert_eq!(w.fake.tag_names("/Docs/invoice.txt"), ["Invoice"]);
    assert_eq!(w.history.next_undo().unwrap().kind, domain::OperationKind::Organize);

    let undo = UndoOperation::new(w.mounts.clone(), w.history.clone());
    block_on(undo.execute(&w.ctx())).unwrap().result.unwrap();
    assert!(w.fake.contents("/Docs/scan0001.pdf").is_some());
    assert!(w.fake.contents("/Docs/old.dmg").is_some());
    assert!(w.fake.tag_names("/Docs/invoice.txt").is_empty());
}

fn configure_cloud(w: &World, folder: &str) {
    let config = CloudModelConfig {
        provider: CloudProvider::Anthropic,
        base_url: "https://api.anthropic.com".into(),
        model: "claude-sonnet-5-5".into(),
    };
    let setup = w.setup();
    block_on(setup.configure(config, Some(Secret::new("sk-test".into())))).unwrap();
    block_on(setup.set_opt_in(&w.fake.path(folder), true)).unwrap();
}

fn proposal() -> ModelProposal {
    ModelProposal {
        groups: vec![
            ProposedGroup { folder: "Housing".into(), item_ids: vec![0, 1, 99], reason: "About the flat".into() },
            ProposedGroup { folder: "bad/name".into(), item_ids: vec![0], reason: String::new() },
        ],
        renames: vec![ProposedRename { item_id: 0, name: "Lease 2026".into(), reason: "Clearer".into() }],
        tags: vec![ProposedTags { item_id: 1, tags: vec!["Home".into(), "a,b".into()] }],
    }
}

#[test]
fn the_cloud_model_is_only_asked_about_opted_in_folders_and_only_gets_summaries() {
    let w = World::new(Ok(proposal()));
    w.fake.add_dir("/Docs");
    w.fake.add_dir("/Docs/Old stuff");
    w.file("/Docs/scan0001.pdf", &format!("Lease agreement {}", "x".repeat(2000)), 3);
    w.file("/Docs/rent.txt", "Rent for March", 2);
    w.analyze("/Docs", CloudUse::IfOptedIn);
    assert!(w.model.requests().is_empty(), "not opted in yet");

    configure_cloud(&w, "/Docs");
    let analysis = w.analyze("/Docs", CloudUse::IfOptedIn);
    assert!(analysis.used_cloud);
    let (config, key, request) = w.model.requests().pop().unwrap();
    assert_eq!((config.model.as_str(), key.as_str()), ("claude-sonnet-5-5", "sk-test"));
    assert_eq!(request.folder_name, "Docs", "the name, never the path");
    let lease = request.items.iter().find(|i| i.name == "scan0001.pdf").unwrap();
    assert!(lease.excerpt.as_ref().unwrap().chars().count() <= EXCERPT_CHARS);
    let folder = request.items.iter().find(|i| i.name == "Old stuff").unwrap();
    assert_eq!((folder.kind.as_str(), folder.excerpt.as_ref()), ("folder", None));

    let preview = block_on(
        PreviewCloudRequest::new(w.mounts.clone(), Arc::new(FakeExtractor::new(w.fake.clone())))
            .execute(&w.fake.path("/Docs"), CLOCK),
    )
    .unwrap();
    assert_eq!(preview, request, "the preview is exactly what's sent");

    let from_cloud: Vec<_> =
        analysis.plan.suggestions.iter().filter(|s| s.source == SuggestionSource::CloudModel).collect();
    let item = |id: u32| request.items[id as usize].name.clone();
    let extension = item(0).rsplit_once('.').map(|(_, e)| e.to_owned()).unwrap();
    let renamed = format!("Lease 2026.{extension}");
    assert!(
        from_cloud.iter().any(|s| matches!(&s.change, Change::Rename(n) if n.display_name() == renamed)),
        "extension kept"
    );
    assert!(from_cloud.iter().all(|s| !matches!(&s.change, Change::MoveInto(n) if n.display_name().contains('/'))));
    let housed: Vec<String> = from_cloud
        .iter()
        .filter(|s| matches!(&s.change, Change::MoveInto(n) if n.display_name() == "Housing"))
        .map(|s| s.item.name().unwrap().display_name().into_owned())
        .collect();
    assert_eq!(housed.len(), 2, "unknown id 99 dropped");
    assert!(housed.contains(&item(0)));
    let tags: Vec<_> = from_cloud
        .iter()
        .filter_map(|s| if let Change::AddTags(t) = &s.change { Some(t.clone()) } else { None })
        .collect();
    assert_eq!(tags.concat().iter().map(|t| t.as_str()).collect::<Vec<_>>(), ["Home"], "invalid tag dropped");
}

#[test]
fn cloud_failures_fall_back_to_local_suggestions_and_forgetting_removes_everything() {
    let w = World::new(Err(ModelError::Unauthorized));
    w.fake.add_dir("/Docs");
    w.file("/Docs/scan0001.pdf", "Lease agreement for the flat", 3);
    configure_cloud(&w, "/Docs");
    let analysis = w.analyze("/Docs", CloudUse::IfOptedIn);
    assert!(!analysis.used_cloud);
    assert!(analysis.notes.iter().any(|n| n.contains("API key was refused")));
    assert!(!changes_for(&analysis.plan, "scan0001.pdf").is_empty(), "local rules still ran");

    assert!(block_on(w.setup().is_opted_in(&w.fake.path("/Docs"))).unwrap());
    block_on(w.setup().forget()).unwrap();
    let status = block_on(w.setup().status()).unwrap();
    assert!(status.settings.is_none() && !status.has_key);
    assert!(block_on(w.keychain.load(&application::ports::CredentialKey::cloud_model_key())).unwrap().is_none());
}

#[test]
fn tags_are_never_suggested_where_they_cant_be_stored() {
    let w = World::new(Ok(ModelProposal::default()));
    w.remote.add_dir("/bucket");
    w.remote.add_file_with("/bucket/bill.txt", b"Invoice 42");
    let analysis = block_on(w.analyzer(&w.remote).execute(
        &w.remote.path("/bucket"),
        CLOCK,
        CloudUse::Never,
        Arc::new(NullObserver),
        CancelToken::never(),
    ))
    .unwrap();
    assert!(analysis.plan.suggestions.iter().all(|s| !matches!(s.change, Change::AddTags(_))));
}
