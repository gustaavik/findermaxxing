//! Fakes of the M7 utility ports: a toy hash, a toy archive format, toy
//! image tools, a terminal that echoes, and `FakeVfs` permission editing
//! and secure erasing.

use std::sync::{Arc, Mutex, PoisonError};

use domain::{
    ArchiveFormat, ByteSize, ConversionRequest, Digest, HashAlgorithm, ImageFingerprint, MetadataField, StoragePath,
    UnixMode, VfsError, entry_path, sensitivity_of,
};
use futures::future::{self, BoxFuture};
use futures::stream::{self, BoxStream};
use futures::{FutureExt, StreamExt};

use super::{FAKE_LOCAL_ROOT, FakeVfs};
use crate::cancel::CancelToken;
use crate::ports::{
    ArchiveBuilder, ArchiveError, ArchiveEvent, ArchiveExtractor, CellPoint, ContentHasher, CursorShape,
    ImageConverter, ImageError, ImageFingerprinter, ImageMetadata, KeyModifiers, LocalFile, PermissionEditor,
    ProgressCallback, RunningHash, ScreenChanged, SecureEraser, SkipReason, StyledRun, TerminalColor, TerminalError,
    TerminalKey, TerminalLauncher, TerminalScreen, TerminalSession, TerminalSize, TextStyle,
};

impl PermissionEditor for FakeVfs {
    fn set_permissions<'a>(&'a self, path: &'a StoragePath, mode: UnixMode) -> BoxFuture<'a, Result<(), VfsError>> {
        let result = self.check_failure(path).and_then(|()| {
            let mut state = self.lock();
            let node = state.nodes.get_mut(path).ok_or(VfsError::NotFound)?;
            node.metadata.permissions = Some(mode);
            Ok(())
        });
        future::ready(result).boxed()
    }
}

impl SecureEraser for FakeVfs {
    fn erase<'a>(
        &'a self,
        file: &'a StoragePath,
        passes: u8,
        progress: ProgressCallback,
        _cancel: CancelToken,
    ) -> BoxFuture<'a, Result<(), VfsError>> {
        let result = self.check_failure(file).and_then(|()| {
            let node = self.node(file)?;
            if node.kind != domain::EntryKind::File {
                return Err(VfsError::IsADirectory);
            }
            progress(ByteSize(node.content.len() as u64 * u64::from(passes)));
            self.remove_tree(file);
            self.lock().erased.push(file.clone());
            Ok(())
        });
        future::ready(result).boxed()
    }
}

impl FakeVfs {
    pub fn set_permissions(&self, absolute: &str, mode: u32) {
        let path = self.path(absolute);
        if let Some(node) = self.lock().nodes.get_mut(&path) {
            node.metadata.permissions = Some(UnixMode::from_mode(mode));
        }
    }

    #[must_use]
    pub fn permissions(&self, absolute: &str) -> Option<u32> {
        self.lock().nodes.get(&self.path(absolute)).and_then(|n| n.metadata.permissions).map(UnixMode::bits)
    }

    #[must_use]
    pub fn erased(&self) -> Vec<StoragePath> {
        self.lock().erased.clone()
    }

    /// The fake contents behind a materialised local file.
    fn local_contents(&self, file: &LocalFile) -> Option<Vec<u8>> {
        let prefix = format!("{FAKE_LOCAL_ROOT}/{}", self.traits.scheme);
        let path = file.path().to_string_lossy().into_owned();
        path.strip_prefix(&prefix).and_then(|absolute| self.contents(absolute))
    }
}

/// Not a real hash: FNV-1a stretched to the algorithm's length. Equal input,
/// equal digest; that's all the use cases rely on.
#[derive(Debug, Default)]
pub struct FakeHasher;

struct FakeRunningHash {
    algorithm: HashAlgorithm,
    state: u64,
}

impl RunningHash for FakeRunningHash {
    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(self: Box<Self>) -> Digest {
        let bytes: Vec<u8> =
            self.state.to_be_bytes().iter().copied().cycle().take(self.algorithm.hex_len() / 2).collect();
        Digest::from_bytes(self.algorithm, &bytes)
    }
}

impl ContentHasher for FakeHasher {
    fn start(&self, algorithm: HashAlgorithm) -> Box<dyn RunningHash> {
        Box::new(FakeRunningHash { algorithm, state: 0xcbf2_9ce4_8422_2325 })
    }
}

/// A toy archive: `D\t<path>\n` for folders, `F\t<path>\t<length>\n<bytes>`
/// for files. Paths go through the real safety rule.
#[must_use]
pub fn fake_archive(entries: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
    let mut archive = Vec::new();
    for (path, content) in entries {
        match content {
            None => archive.extend_from_slice(format!("D\t{path}\n").as_bytes()),
            Some(bytes) => {
                archive.extend_from_slice(format!("F\t{path}\t{}\n", bytes.len()).as_bytes());
                archive.extend_from_slice(bytes);
            }
        }
    }
    archive
}

fn parse_fake_archive(mut bytes: &[u8]) -> Result<Vec<ArchiveEvent>, String> {
    let mut events = Vec::new();
    while !bytes.is_empty() {
        let end = bytes.iter().position(|b| *b == b'\n').ok_or("header without newline")?;
        let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
        bytes = &bytes[end + 1..];
        let parts: Vec<&str> = header.split('\t').collect();
        let path = parts.get(1).ok_or("header without path")?;
        let content = match parts.first() {
            Some(&"F") => {
                let length: usize = parts.get(2).and_then(|l| l.parse().ok()).ok_or("bad length")?;
                let (content, rest) = bytes.split_at(length.min(bytes.len()));
                bytes = rest;
                Some(content.to_vec())
            }
            _ => None,
        };
        match (entry_path(path.as_bytes()), content) {
            (Err(reason), _) => {
                events.push(ArchiveEvent::Skipped { name: (*path).to_owned(), reason: SkipReason::Unsafe(reason) })
            }
            (Ok(path), None) => events.push(ArchiveEvent::Folder(path)),
            (Ok(path), Some(content)) => {
                events.push(ArchiveEvent::File {
                    path,
                    size: Some(ByteSize(content.len() as u64)),
                    mode: None,
                    modified: None,
                });
                events.extend(content.chunks(3).map(|c| ArchiveEvent::Chunk(c.to_vec())));
                events.push(ArchiveEvent::EndFile);
            }
        }
    }
    Ok(events)
}

/// Reads and writes [`fake_archive`]s; RAR is refused for writing, as for real.
#[derive(Debug)]
pub struct FakeArchiver {
    fake: FakeVfs,
}

impl FakeArchiver {
    #[must_use]
    pub fn new(fake: FakeVfs) -> Self {
        Self { fake }
    }
}

impl ArchiveExtractor for FakeArchiver {
    fn extract(
        &self,
        archive: LocalFile,
        format: ArchiveFormat,
        _cancel: CancelToken,
    ) -> BoxStream<'static, Result<ArchiveEvent, ArchiveError>> {
        let parsed = self
            .fake
            .local_contents(&archive)
            .ok_or(ArchiveError::Io(VfsError::NotFound))
            .and_then(|bytes| parse_fake_archive(&bytes).map_err(|e| ArchiveError::Corrupt(format, e)));
        match parsed {
            Ok(events) => stream::iter(events.into_iter().map(Ok)).boxed(),
            Err(error) => stream::once(future::ready(Err(error))).boxed(),
        }
    }
}

impl ArchiveBuilder for FakeArchiver {
    fn build(
        &self,
        format: ArchiveFormat,
        contents: BoxStream<'static, ArchiveEvent>,
        _cancel: CancelToken,
    ) -> BoxStream<'static, Result<Vec<u8>, ArchiveError>> {
        if !format.can_create() {
            return stream::once(future::ready(Err(ArchiveError::CannotCreate(format)))).boxed();
        }
        let packed = contents.collect::<Vec<_>>().map(|events| {
            let mut archive = Vec::new();
            let mut file: Option<(String, Vec<u8>)> = None;
            for event in events {
                match event {
                    ArchiveEvent::Folder(path) => archive.extend(fake_archive(&[(&joined(&path), None)])),
                    ArchiveEvent::File { path, .. } => file = Some((joined(&path), Vec::new())),
                    ArchiveEvent::Chunk(bytes) => {
                        if let Some((_, content)) = file.as_mut() {
                            content.extend(bytes);
                        }
                    }
                    ArchiveEvent::EndFile => {
                        if let Some((path, content)) = file.take() {
                            archive.extend(fake_archive(&[(&path, Some(&content))]));
                        }
                    }
                    ArchiveEvent::Skipped { .. } => {}
                }
            }
            Ok(archive)
        });
        stream::once(packed).boxed()
    }
}

fn joined(path: &[domain::PathSegment]) -> String {
    path.iter().map(|s| s.display_name().into_owned()).collect::<Vec<_>>().join("/")
}

/// Toy images: the bytes are text. Converting prefixes the format; the
/// fingerprint is the first eight bytes; lines `EXIF:Tag=Value` are metadata.
#[derive(Debug)]
pub struct FakeImageTools {
    fake: FakeVfs,
}

impl FakeImageTools {
    #[must_use]
    pub fn new(fake: FakeVfs) -> Self {
        Self { fake }
    }

    fn read(&self, image: &LocalFile) -> Result<Vec<u8>, ImageError> {
        self.fake.local_contents(image).ok_or_else(|| ImageError::Unreadable("missing".into()))
    }
}

const FAKE_EXIF_PREFIX: &str = "EXIF:";

impl ImageConverter for FakeImageTools {
    fn convert(&self, image: LocalFile, request: ConversionRequest) -> BoxFuture<'static, Result<Vec<u8>, ImageError>> {
        let converted = self.read(&image).map(|bytes| {
            let mut out = format!("{}:", request.format.extension()).into_bytes();
            out.extend(bytes);
            out
        });
        future::ready(converted).boxed()
    }
}

impl ImageFingerprinter for FakeImageTools {
    fn fingerprint(&self, image: LocalFile) -> BoxFuture<'static, Option<ImageFingerprint>> {
        let fingerprint = self.read(&image).ok().map(|bytes| ImageFingerprint(bytes.into_iter().take(8).collect()));
        future::ready(fingerprint).boxed()
    }
}

impl ImageMetadata for FakeImageTools {
    fn read(&self, image: LocalFile) -> BoxFuture<'static, Result<Vec<MetadataField>, ImageError>> {
        let fields = FakeImageTools::read(self, &image).map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .lines()
                .filter_map(|line| line.strip_prefix(FAKE_EXIF_PREFIX)?.split_once('='))
                .map(|(tag, value)| MetadataField {
                    group: "Exif".into(),
                    tag: tag.into(),
                    value: value.into(),
                    sensitivity: sensitivity_of(tag),
                })
                .collect()
        });
        future::ready(fields).boxed()
    }

    fn strip(&self, image: LocalFile) -> BoxFuture<'static, Result<Vec<u8>, ImageError>> {
        let stripped = FakeImageTools::read(self, &image).map(|bytes| {
            let text = String::from_utf8_lossy(&bytes);
            text.lines().filter(|l| !l.starts_with(FAKE_EXIF_PREFIX)).collect::<Vec<_>>().join("\n").into_bytes()
        });
        future::ready(stripped).boxed()
    }
}

/// Starts [`EchoTerminal`]s and remembers the folders they started in.
#[derive(Debug, Default)]
pub struct FakeTerminalLauncher {
    pub started_in: Mutex<Vec<std::path::PathBuf>>,
}

impl TerminalLauncher for FakeTerminalLauncher {
    fn open(
        &self,
        folder: std::path::PathBuf,
        size: TerminalSize,
        changed: ScreenChanged,
    ) -> Result<Arc<dyn TerminalSession>, TerminalError> {
        self.started_in.lock().unwrap_or_else(PoisonError::into_inner).push(folder);
        Ok(Arc::new(EchoTerminal { typed: Mutex::new(String::new()), size: Mutex::new(size), changed }))
    }
}

/// Shows what was typed on its first row.
struct EchoTerminal {
    typed: Mutex<String>,
    size: Mutex<TerminalSize>,
    changed: ScreenChanged,
}

impl TerminalSession for EchoTerminal {
    fn key(&self, key: TerminalKey, _modifiers: KeyModifiers) {
        if let TerminalKey::Char(c) = key {
            self.paste(&c.to_string());
        }
    }

    fn paste(&self, text: &str) {
        self.typed.lock().unwrap_or_else(PoisonError::into_inner).push_str(text);
        (self.changed)();
    }

    fn resize(&self, size: TerminalSize) {
        *self.size.lock().unwrap_or_else(PoisonError::into_inner) = size;
    }

    fn screen(&self) -> TerminalScreen {
        let typed = self.typed.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let size = *self.size.lock().unwrap_or_else(PoisonError::into_inner);
        let cells = u16::try_from(typed.chars().count()).unwrap_or(u16::MAX);
        let run = StyledRun {
            column: 0,
            text: typed,
            cells,
            foreground: TerminalColor::Foreground,
            background: TerminalColor::Background,
            style: TextStyle::default(),
        };
        TerminalScreen {
            size,
            rows: vec![vec![run]],
            cursor_row: 0,
            cursor_column: cells,
            cursor: CursorShape::Block,
            title: String::new(),
            scrolled_back: 0,
        }
    }

    fn scroll(&self, _lines: i32) {}

    fn select(&self, _from: CellPoint, _to: CellPoint) {}

    fn clear_selection(&self) {}

    fn selected_text(&self) -> Option<String> {
        None
    }

    fn follow(&self, _folder: &std::path::Path) -> bool {
        false
    }

    fn is_running(&self) -> bool {
        true
    }
}
