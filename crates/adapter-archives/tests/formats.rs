//! Every writable format round-trips through `LocalArchiver`; archives open
//! in the system's own tools; unsafe entries and encrypted archives are
//! handled; RAR extracts through unRAR.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use adapter_archives::LocalArchiver;
use application::CancelToken;
use application::ports::{ArchiveBuilder, ArchiveError, ArchiveEvent, ArchiveExtractor, LocalFile, SkipReason};
use domain::{ArchiveFormat, ByteSize, CREATABLE_FORMATS, PathSegment, Timestamp, UnixMode, UnsafeEntry};

/// 2026-03-14 15:09:26 UTC.
const MODIFIED_SECONDS: i64 = 1_773_500_966;
use futures::{StreamExt, TryStreamExt};

fn segments(path: &str) -> Vec<PathSegment> {
    path.split('/').map(|s| PathSegment::new(s).unwrap()).collect()
}

fn big_file() -> Vec<u8> {
    (0..700_000_u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect()
}

fn contents() -> Vec<ArchiveEvent> {
    let big = big_file();
    let mut events = vec![
        ArchiveEvent::Folder(segments("docs")),
        ArchiveEvent::File {
            path: segments("docs/run.sh"),
            size: Some(ByteSize(9)),
            mode: Some(UnixMode::from_mode(0o755)),
            modified: Some(Timestamp::from_unix_millis(MODIFIED_SECONDS * 1000)),
        },
        ArchiveEvent::Chunk(b"echo hi\n".to_vec()),
        ArchiveEvent::Chunk(b"\n".to_vec()),
        ArchiveEvent::EndFile,
        ArchiveEvent::File {
            path: segments("big.bin"),
            size: Some(ByteSize(big.len() as u64)),
            mode: None,
            modified: None,
        },
    ];
    events.extend(big.chunks(100_000).map(|c| ArchiveEvent::Chunk(c.to_vec())));
    events.push(ArchiveEvent::EndFile);
    events
}

#[derive(Debug, Default, PartialEq)]
struct Extracted {
    folders: Vec<String>,
    files: BTreeMap<String, (Vec<u8>, Option<u32>)>,
    skipped: Vec<(String, SkipReason)>,
}

async fn extract(archiver: &LocalArchiver, archive: &Path, format: ArchiveFormat) -> Result<Extracted, ArchiveError> {
    let events: Vec<ArchiveEvent> =
        archiver.extract(LocalFile::new(archive.to_path_buf()), format, CancelToken::never()).try_collect().await?;
    let mut out = Extracted::default();
    let mut current: Option<(String, Vec<u8>, Option<u32>)> = None;
    let name = |p: &[PathSegment]| p.iter().map(|s| s.display_name().into_owned()).collect::<Vec<_>>().join("/");
    for event in events {
        match event {
            ArchiveEvent::Folder(path) => out.folders.push(name(&path)),
            ArchiveEvent::File { path, mode, .. } => {
                current = Some((name(&path), Vec::new(), mode.map(UnixMode::bits)))
            }
            ArchiveEvent::Chunk(bytes) => current.as_mut().unwrap().1.extend(bytes),
            ArchiveEvent::EndFile => {
                let (path, data, mode) = current.take().unwrap();
                out.files.insert(path, (data, mode));
            }
            ArchiveEvent::Skipped { name, reason } => out.skipped.push((name, reason)),
        }
    }
    Ok(out)
}

async fn build(archiver: &LocalArchiver, format: ArchiveFormat, dir: &Path) -> PathBuf {
    let bytes: Vec<Vec<u8>> = archiver
        .build(format, futures::stream::iter(contents()).boxed(), CancelToken::never())
        .try_collect()
        .await
        .unwrap();
    let path = dir.join(format!("test{}", format.extension()));
    std::fs::write(&path, bytes.concat()).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn every_writable_format_round_trips_with_modes() {
    let archiver = LocalArchiver::new(tokio::runtime::Handle::current());
    let dir = tempfile::tempdir().unwrap();
    for format in CREATABLE_FORMATS {
        let path = build(&archiver, format, dir.path()).await;
        let out = extract(&archiver, &path, format).await.unwrap();
        assert_eq!(out.folders, ["docs"], "{format:?}");
        assert_eq!(out.files["docs/run.sh"].0, b"echo hi\n\n", "{format:?}");
        assert_eq!(out.files["big.bin"].0, big_file(), "{format:?}");
        let mode = out.files["docs/run.sh"].1.unwrap() & 0o777;
        assert_eq!(mode, 0o755, "{format:?} keeps the execute bits");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn modification_times_are_kept() {
    let archiver = LocalArchiver::new(tokio::runtime::Handle::current());
    let dir = tempfile::tempdir().unwrap();
    let tar = build(&archiver, ArchiveFormat::Tar, dir.path()).await;
    let mut archive = tar::Archive::new(std::fs::File::open(&tar).unwrap());
    let script =
        archive.entries().unwrap().map(Result::unwrap).find(|e| e.path().unwrap().ends_with("run.sh")).unwrap();
    assert_eq!(script.header().mtime().unwrap(), u64::try_from(MODIFIED_SECONDS).unwrap());
    let zip_path = build(&archiver, ArchiveFormat::Zip, dir.path()).await;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
    let modified = zip.by_name("docs/run.sh").unwrap().last_modified().unwrap();
    assert_eq!(
        (modified.year(), modified.month(), modified.day(), modified.hour(), modified.minute()),
        (2026, 3, 14, 15, 9)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn archives_open_in_the_system_tools() {
    let archiver = LocalArchiver::new(tokio::runtime::Handle::current());
    let dir = tempfile::tempdir().unwrap();
    let checks: [(ArchiveFormat, &str, &[&str]); 3] = [
        (ArchiveFormat::Zip, "7z", &["t"]),
        (ArchiveFormat::SevenZ, "7z", &["t"]),
        (ArchiveFormat::TarXz, "bsdtar", &["-tf"]),
    ];
    for (format, tool, args) in checks {
        if Command::new(tool).arg("--help").output().is_err() {
            continue;
        }
        let path = build(&archiver, format, dir.path()).await;
        let output = Command::new(tool).args(args).arg(&path).output().unwrap();
        assert!(output.status.success(), "{tool} on {format:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn zip_slip_entries_are_skipped() {
    let archiver = LocalArchiver::new(tokio::runtime::Handle::current());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("evil.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("../../evil.txt", options).unwrap();
    zip.write_all(b"evil").unwrap();
    zip.start_file("fine.txt", options).unwrap();
    zip.write_all(b"fine").unwrap();
    zip.finish().unwrap();
    let out = extract(&archiver, &path, ArchiveFormat::Zip).await.unwrap();
    assert_eq!(out.skipped, [("../../evil.txt".to_owned(), SkipReason::Unsafe(UnsafeEntry::EscapesFolder))]);
    assert_eq!(out.files["fine.txt"].0, b"fine");
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

#[tokio::test(flavor = "multi_thread")]
async fn rar_archives_extract_through_unrar() {
    let archiver = LocalArchiver::new(tokio::runtime::Handle::current());
    let out = extract(&archiver, &fixture("solid.rar"), ArchiveFormat::Rar).await.unwrap();
    assert_eq!(out.files[".gitignore"].0.len(), 18);
    let locked = extract(&archiver, &fixture("crypted.rar"), ArchiveFormat::Rar).await;
    assert_eq!(locked, Err(ArchiveError::Encrypted));
}

#[tokio::test(flavor = "multi_thread")]
async fn rar_is_never_written_and_garbage_is_reported_as_damaged() {
    let archiver = LocalArchiver::new(tokio::runtime::Handle::current());
    let refused: Vec<_> =
        archiver.build(ArchiveFormat::Rar, futures::stream::empty().boxed(), CancelToken::never()).collect().await;
    assert_eq!(refused, [Err(ArchiveError::CannotCreate(ArchiveFormat::Rar))]);
    let dir = tempfile::tempdir().unwrap();
    let garbage = dir.path().join("x.zip");
    std::fs::write(&garbage, b"not a zip at all").unwrap();
    assert!(matches!(extract(&archiver, &garbage, ArchiveFormat::Zip).await, Err(ArchiveError::Corrupt(..))));
}
