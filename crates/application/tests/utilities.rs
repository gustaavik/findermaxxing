//! The M7a utilities against fakes: checksums, duplicates, disk usage,
//! archives, image tools, permissions, secure delete and the terminal.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::path::PathBuf;
use std::sync::Arc;

use application::operations::{JobContext, NullObserver, OperationError, OperationHistory};
use application::ports::{MountResolver, TerminalSize};
use application::testing::{
    FakeArchiver, FakeHasher, FakeImageTools, FakeTerminalLauncher, FakeVfs, RecordingObserver, fake_archive,
};
use application::use_cases::{
    ChangePermissions, CleanImageMetadata, CompressItems, CompressRequest, ComputeChecksums, ConvertImages,
    ConvertRequest, DuplicateSearch, ExtractArchive, ExtractRequest, FindDuplicates, FindPublishedChecksum,
    GetPermissions, MeasureDiskUsage, OpenTerminal, ReadImageMetadata, SecureDelete, UndoOperation,
};
use application::{CancelToken, ProviderRegistry};
use domain::{
    Access, ArchiveFormat, BitChange, ByteSize, ConversionRequest, DuplicateMatch, HashAlgorithm, ImageFormat,
    PermissionClass, PermissionEdit, PermissionReach, Resize, Sensitivity, UsageKind, Verification, VfsError, verify,
};
use futures_executor::block_on;

struct World {
    fake: FakeVfs,
    remote: FakeVfs,
    mounts: Arc<dyn MountResolver>,
    history: Arc<OperationHistory>,
}

impl World {
    fn new() -> Self {
        let fake = FakeVfs::new();
        let remote = FakeVfs::remote("s3", Some("eu-west-1"));
        let registry = ProviderRegistry::default()
            .with_provider(fake.clone().into_factory())
            .unwrap()
            .with_provider(remote.clone().into_factory())
            .unwrap();
        Self { fake, remote, mounts: Arc::new(registry), history: Arc::default() }
    }

    fn ctx(&self) -> JobContext {
        JobContext::unattended(Arc::new(RecordingObserver::default()), CancelToken::never())
    }

    fn undo(&self) {
        let undo = UndoOperation::new(self.mounts.clone(), self.history.clone());
        block_on(undo.execute(&self.ctx())).unwrap().result.unwrap();
    }

    fn content(&self, absolute: &str) -> Option<String> {
        self.fake.contents(absolute).map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

#[test]
fn checksums_are_computed_in_one_pass_and_checked_against_a_published_list() {
    let w = World::new();
    w.fake.add_dir("/dl");
    w.fake.add_file_with("/dl/app.dmg", b"installer bytes");
    w.fake.add_file_with("/dl/copy.dmg", b"installer bytes");
    let checksums = ComputeChecksums::new(w.mounts.clone(), Arc::new(FakeHasher));
    let all = HashAlgorithm::ALL;
    let digests =
        block_on(checksums.execute(&w.fake.path("/dl/app.dmg"), &all, Arc::new(|_| {}), CancelToken::never())).unwrap();
    assert_eq!(digests.iter().map(|d| d.algorithm()).collect::<Vec<_>>(), all);
    let copy = block_on(checksums.execute(&w.fake.path("/dl/copy.dmg"), &all, Arc::new(|_| {}), CancelToken::never()))
        .unwrap();
    assert_eq!(digests, copy, "same bytes, same digests");

    let published = FindPublishedChecksum::new(w.mounts.clone());
    assert_eq!(block_on(published.execute(&w.fake.path("/dl/app.dmg"))).unwrap(), None);
    let sha = digests[2].hex().to_owned();
    w.fake.add_file_with("/dl/SHA256SUMS", format!("0000  other.iso\n{sha}  app.dmg\n").as_bytes());
    let found = block_on(published.execute(&w.fake.path("/dl/app.dmg"))).unwrap().unwrap();
    assert_eq!(verify(&found, &digests), Verification::Matches(HashAlgorithm::Sha256));
    w.fake.add_file_with("/dl/copy.dmg.sha256", "f".repeat(64).as_bytes());
    let own = block_on(published.execute(&w.fake.path("/dl/copy.dmg"))).unwrap().unwrap();
    assert_eq!(verify(&own, &copy), Verification::Differs);
}

fn search(w: &World, folders: &[&str]) -> DuplicateSearch {
    DuplicateSearch {
        folders: folders.iter().map(|f| w.fake.path(f)).collect(),
        min_size: ByteSize(1),
        include_hidden: false,
        similar_images: false,
        top_level_only: false,
    }
}

#[test]
fn duplicates_are_found_by_content_and_the_oldest_copy_is_kept() {
    let w = World::new();
    for folder in ["/a", "/a/old", "/b"] {
        w.fake.add_dir(folder);
    }
    let big: Vec<u8> = (0..200_000_u32).map(|i| (i % 251) as u8).collect();
    let mut big_variant = big.clone();
    *big_variant.last_mut().unwrap() ^= 1;
    w.fake.add_file_with("/a/old/report.pdf", &big);
    w.fake.add_file_with("/b/report copy.pdf", &big);
    w.fake.add_file_with("/b/report v2.pdf", &big_variant);
    w.fake.add_file_with("/a/x.txt", b"same");
    w.fake.add_file_with("/b/y.txt", b"diff");
    w.fake.add_file_with("/b/.hidden.txt", b"same");
    w.fake.set_modified("/a/old/report.pdf", domain::Timestamp::from_unix_millis(1));
    w.fake.set_modified("/b/report copy.pdf", domain::Timestamp::from_unix_millis(2));
    let finder =
        FindDuplicates::new(w.mounts.clone(), Arc::new(FakeHasher), Arc::new(FakeImageTools::new(w.fake.clone())));
    let groups =
        block_on(finder.execute(&search(&w, &["/a", "/b", "/a/old"]), Arc::new(NullObserver), CancelToken::never()))
            .unwrap();
    assert_eq!(groups.len(), 1, "same size but different end; hidden left out; nested folder not doubled");
    let names: Vec<String> = groups[0].items.iter().map(|i| i.path().path_string()).collect();
    assert_eq!(names, ["/a/old/report.pdf", "/b/report copy.pdf"]);
    assert_eq!(groups[0].matching, DuplicateMatch::Identical);
    assert_eq!(groups[0].reclaimable(), ByteSize(200_000));

    let mut with_hidden = search(&w, &["/a", "/b"]);
    with_hidden.include_hidden = true;
    let groups = block_on(finder.execute(&with_hidden, Arc::new(NullObserver), CancelToken::never())).unwrap();
    assert_eq!(groups.len(), 2);
}

#[test]
fn look_alike_images_form_similar_groups() {
    let w = World::new();
    w.fake.add_dir("/p");
    w.fake.add_file_with("/p/beach.jpg", b"SUNSET01-full-size");
    w.fake.add_file_with("/p/beach small.png", b"SUNSET01-small");
    w.fake.add_file_with("/p/cat.jpg", b"CATCATCA-whatever");
    let finder =
        FindDuplicates::new(w.mounts.clone(), Arc::new(FakeHasher), Arc::new(FakeImageTools::new(w.fake.clone())));
    let mut request = search(&w, &["/p"]);
    request.similar_images = true;
    let groups = block_on(finder.execute(&request, Arc::new(NullObserver), CancelToken::never())).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].matching, DuplicateMatch::Similar);
    assert_eq!(groups[0].items.len(), 2);
}

#[test]
fn disk_usage_sums_folders_and_keeps_unreadable_ones_visible() {
    let w = World::new();
    for folder in ["/home", "/home/Movies", "/home/Locked"] {
        w.fake.add_dir(folder);
    }
    w.fake.add_file("/home/Movies/film.mkv", 7000);
    w.fake.add_file("/home/notes.txt", 300);
    w.fake.fail_listing("/home/Locked", VfsError::PermissionDenied);
    let counted = Arc::new(std::sync::Mutex::new(0));
    let seen = counted.clone();
    let usage = MeasureDiskUsage::new(w.mounts.clone());
    let progress: application::use_cases::UsageProgress = Arc::new(move |files, _| *seen.lock().unwrap() = files);
    let tree = block_on(usage.execute(&w.fake.path("/home"), progress, CancelToken::never())).unwrap();
    assert_eq!(tree.size, ByteSize(7300));
    assert_eq!(tree.files, 2);
    assert_eq!(tree.children[0].path, w.fake.path("/home/Movies"));
    assert!(tree.children.iter().any(|c| c.kind == UsageKind::Unreadable));
    assert!(*counted.lock().unwrap() >= 1);
}

#[test]
fn compressing_and_extracting_round_trip_and_both_undo() {
    let w = World::new();
    for folder in ["/w", "/w/Photos", "/w/Photos/2026"] {
        w.fake.add_dir(folder);
    }
    w.fake.add_file_with("/w/Photos/2026/a.jpg", b"first image");
    w.fake.add_file_with("/w/Photos/b.jpg", b"second");
    let archiver = Arc::new(FakeArchiver::new(w.fake.clone()));
    let compress = CompressItems::new(w.mounts.clone(), w.history.clone(), archiver.clone());
    let request = CompressRequest {
        items: vec![w.fake.path("/w/Photos")],
        format: ArchiveFormat::Zip,
        destination: w.fake.path("/w"),
    };
    block_on(compress.execute(&request, &w.ctx())).result.unwrap();
    assert!(w.fake.contents("/w/Photos.zip").is_some());
    block_on(compress.execute(&request, &w.ctx())).result.unwrap();
    assert!(w.fake.contents("/w/Photos 2.zip").is_some(), "a taken name is numbered");

    let extract = ExtractArchive::new(w.mounts.clone(), w.history.clone(), archiver);
    let request = ExtractRequest { archive: w.fake.path("/w/Photos.zip"), destination: w.fake.path("/w") };
    block_on(extract.execute(&request, &w.ctx())).result.unwrap();
    assert_eq!(
        w.content("/w/Photos 2/Photos/2026/a.jpg").as_deref(),
        Some("first image"),
        "`Photos` exists, so `Photos 2`"
    );
    assert_eq!(w.content("/w/Photos 2/Photos/b.jpg").as_deref(), Some("second"));

    w.undo();
    assert!(w.fake.contents("/w/Photos 2/Photos/b.jpg").is_none(), "undo trashes the extracted folder");
    w.undo();
    assert!(w.fake.contents("/w/Photos 2.zip").is_none(), "undo trashes the second archive");
}

#[test]
fn unsafe_archive_entries_are_skipped_and_reported() {
    let w = World::new();
    w.fake.add_dir("/d");
    let archive = fake_archive(&[("../../escape.txt", Some(b"evil")), ("ok/file.txt", Some(b"fine"))]);
    w.fake.add_file_with("/d/bundle.tar.gz", &archive);
    let extract = ExtractArchive::new(w.mounts.clone(), w.history.clone(), Arc::new(FakeArchiver::new(w.fake.clone())));
    let request = ExtractRequest { archive: w.fake.path("/d/bundle.tar.gz"), destination: w.fake.path("/d") };
    let report = block_on(extract.execute(&request, &w.ctx()));
    assert_eq!(report.result, Err(OperationError::SkippedEntries { count: 1 }));
    assert_eq!(w.content("/d/bundle/ok/file.txt").as_deref(), Some("fine"));
    assert!(w.fake.contents("/escape.txt").is_none());
}

#[test]
fn rar_archives_are_never_created() {
    let w = World::new();
    w.fake.add_dir("/d");
    w.fake.add_file_with("/d/x", b"x");
    let compress = CompressItems::new(w.mounts.clone(), w.history.clone(), Arc::new(FakeArchiver::new(w.fake.clone())));
    let request = CompressRequest {
        items: vec![w.fake.path("/d/x")],
        format: ArchiveFormat::Rar,
        destination: w.fake.path("/d"),
    };
    assert!(matches!(block_on(compress.execute(&request, &w.ctx())).result, Err(OperationError::Archive(_))));
}

#[test]
fn images_convert_next_to_the_originals_and_undo_removes_the_copies() {
    let w = World::new();
    w.fake.add_dir("/p");
    w.fake.add_file_with("/p/scan.tiff", b"pixels");
    w.fake.add_file_with("/p/scan.png", b"already there");
    let convert =
        ConvertImages::new(w.mounts.clone(), w.history.clone(), Arc::new(FakeImageTools::new(w.fake.clone())));
    let request = ConvertRequest {
        images: vec![w.fake.path("/p/scan.tiff")],
        conversion: ConversionRequest { format: ImageFormat::Png, resize: Resize::Percent(50), quality: 85 },
        destination: None,
    };
    block_on(convert.execute(&request, &w.ctx())).result.unwrap();
    assert_eq!(w.content("/p/scan 2.png").as_deref(), Some("png:pixels"));
    assert_eq!(w.content("/p/scan.tiff").as_deref(), Some("pixels"), "the original stays");
    w.undo();
    assert!(w.fake.contents("/p/scan 2.png").is_none());
}

#[test]
fn cleaning_metadata_replaces_the_image_and_undo_brings_the_original_back() {
    let w = World::new();
    w.fake.add_dir("/p");
    w.fake.add_file_with("/p/me.jpg", b"pixels\nEXIF:GPSLatitude=55.6\nEXIF:Model=Phone");
    let tools = Arc::new(FakeImageTools::new(w.fake.clone()));
    let fields =
        block_on(ReadImageMetadata::new(w.mounts.clone(), tools.clone()).execute(&w.fake.path("/p/me.jpg"))).unwrap();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].sensitivity, Sensitivity::Location);

    let clean = CleanImageMetadata::new(w.mounts.clone(), w.history.clone(), tools);
    block_on(clean.execute(&[w.fake.path("/p/me.jpg")], &w.ctx())).result.unwrap();
    assert_eq!(w.content("/p/me.jpg").as_deref(), Some("pixels"));
    w.undo();
    assert_eq!(w.content("/p/me.jpg").as_deref(), Some("pixels\nEXIF:GPSLatitude=55.6\nEXIF:Model=Phone"));
}

#[test]
fn permissions_change_across_a_tree_and_undo_restores_each_mode() {
    let w = World::new();
    for folder in ["/s", "/s/bin"] {
        w.fake.add_dir(folder);
        w.fake.set_permissions(folder, 0o755);
    }
    w.fake.add_file("/s/bin/run.sh", 1);
    w.fake.set_permissions("/s/bin/run.sh", 0o644);
    let items = [w.fake.path("/s")];
    let view = block_on(GetPermissions::new(w.mounts.clone()).execute(&items)).unwrap();
    assert!(view.editable && view.has_folders);

    let edit = PermissionEdit::default().with(PermissionClass::Others, Access::Read, BitChange::Clear).with(
        PermissionClass::Others,
        Access::Execute,
        BitChange::Clear,
    );
    let change = ChangePermissions::new(w.mounts.clone(), w.history.clone());
    block_on(change.execute(&items, edit, PermissionReach::Everything, &w.ctx())).result.unwrap();
    assert_eq!(w.fake.permissions("/s"), Some(0o750));
    assert_eq!(w.fake.permissions("/s/bin"), Some(0o750));
    assert_eq!(w.fake.permissions("/s/bin/run.sh"), Some(0o640));
    w.undo();
    assert_eq!(w.fake.permissions("/s/bin/run.sh"), Some(0o644));
    assert_eq!(w.fake.permissions("/s"), Some(0o755));

    let files_only = PermissionEdit::default().with(PermissionClass::Owner, Access::Execute, BitChange::Set);
    block_on(change.execute(&items, files_only, PermissionReach::AlsoFilesInside, &w.ctx())).result.unwrap();
    assert_eq!(w.fake.permissions("/s/bin/run.sh"), Some(0o744));
    assert_eq!(w.fake.permissions("/s/bin"), Some(0o755), "folders inside untouched");
}

#[test]
fn secure_delete_erases_files_then_folders_and_refuses_where_it_cannot() {
    let w = World::new();
    w.fake.add_dir("/x");
    w.fake.add_dir("/x/inner");
    w.fake.add_file_with("/x/inner/secret.txt", b"secret");
    w.fake.add_file_with("/x/key.pem", b"key");
    let delete = SecureDelete::new(w.mounts.clone());
    block_on(delete.execute(&[w.fake.path("/x")], &w.ctx())).unwrap();
    assert_eq!(w.fake.erased().len(), 2);
    assert!(w.fake.contents("/x/key.pem").is_none());
    assert!(w.history.next_undo().is_none(), "never undoable");

    w.remote.add_dir("/bucket");
    w.remote.add_file_with("/bucket/a", b"a");
    let refused = block_on(delete.execute(&[w.remote.path("/bucket/a")], &w.ctx()));
    assert!(matches!(refused, Err(OperationError::Failed { error: VfsError::Unsupported(_), .. })));
    assert!(w.remote.contents("/bucket/a").is_some());
}

#[test]
fn the_terminal_starts_in_local_folders_and_at_home_elsewhere() {
    let w = World::new();
    let launcher = Arc::new(FakeTerminalLauncher::default());
    let open = OpenTerminal::new(launcher.clone(), PathBuf::from("/Users/me"));
    let size = TerminalSize { columns: 80, rows: 24 };
    let local = domain::StoragePath::parse("file:///Users/me/Projects").unwrap();
    let session = open.execute(&local, size, Arc::new(|| {})).unwrap();
    session.paste("ls");
    assert_eq!(session.screen().rows[0][0].text, "ls");
    open.execute(&w.remote.path("/bucket"), size, Arc::new(|| {})).unwrap();
    let started = launcher.started_in.lock().unwrap().clone();
    assert_eq!(started, [PathBuf::from("/Users/me/Projects"), PathBuf::from("/Users/me")]);
}
