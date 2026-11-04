//! `ContentExtractor` for local files (ADR 0025). Only the start of a
//! document matters (the index keeps its first passages), so every format
//! stops reading after [`MAX_TEXT_CHARS`].
//!
//! - text and code: read as UTF-8 (lossy), HTML/XML tags stripped;
//! - PDF: `pdf-extract`, page by page, at most [`MAX_PDF_PAGES`];
//! - Word/PowerPoint/Excel (`.docx`/`.pptx`/`.xlsx`), OpenDocument
//!   (`.odt`/`.odp`/`.ods`) and EPUB: the XML inside the zip;
//! - RTF: control words removed.
//!
//! Parsers of untrusted files can panic on malformed input; that is caught
//! and treated as "no text".

use std::io::{Read, Seek};
use std::path::Path;

use application::ports::{ContentExtractor, LocalFile};
use domain::{FileCategory, VfsError};
use futures::FutureExt;
use futures::future::BoxFuture;
use quick_xml::Reader;
use quick_xml::events::Event;
use tokio::runtime::Handle;

/// 48 passages of ~1000 characters, with room for overlap.
pub const MAX_TEXT_CHARS: usize = 64 * 1024;
pub const MAX_PDF_PAGES: u32 = 40;
/// Text files are read up to this many bytes.
const MAX_TEXT_BYTES: u64 = 256 * 1024;
/// A zip member is never inflated beyond this (zip bombs).
const MAX_MEMBER_BYTES: u64 = 8 * 1024 * 1024;
/// Elements whose end is a line break in the extracted text.
const BLOCK_ELEMENTS: &[&str] =
    &["p", "div", "br", "li", "tr", "td", "h1", "h2", "h3", "h4", "h5", "h6", "title", "si", "row", "page"];
/// Elements whose content is never text (scripts, styles).
const SKIPPED_ELEMENTS: &[&str] = &["script", "style", "head"];

#[derive(Debug, Clone)]
pub struct LocalExtractor {
    runtime: Handle,
}

impl LocalExtractor {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }
}

impl ContentExtractor for LocalExtractor {
    fn extract(
        &self,
        file: &LocalFile,
        category: FileCategory,
    ) -> BoxFuture<'static, Result<Option<String>, VfsError>> {
        let path = file.path().to_path_buf();
        let task = self.runtime.spawn_blocking(move || {
            // Malformed input must never take the indexer down.
            std::panic::catch_unwind(|| extract(&path, category)).unwrap_or(Ok(None))
        });
        async move { task.await.map_err(|e| VfsError::Io { message: e.to_string() })? }.boxed()
    }
}

fn io(error: &std::io::Error) -> VfsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound,
        std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied,
        _ => VfsError::Io { message: error.to_string() },
    }
}

/// Extracts by format; `Ok(None)` when there is no text to find.
pub fn extract(path: &Path, category: FileCategory) -> Result<Option<String>, VfsError> {
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let text = match (category, extension.as_str()) {
        (FileCategory::Pdf, _) => pdf(path),
        (_, "docx" | "dotx") => zipped(path, |name| name == "word/document.xml"),
        (_, "pptx") => zipped(path, |name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml")),
        (_, "xlsx") => zipped(path, |name| name == "xl/sharedStrings.xml"),
        (_, "odt" | "ods" | "odp") => zipped(path, |name| name == "content.xml"),
        (_, "epub") => {
            zipped(path, |name| name.ends_with(".xhtml") || name.ends_with(".html") || name.ends_with(".htm"))
        }
        (_, "rtf") => plain(path).map(|text| rtf_text(&text)),
        (_, "html" | "htm" | "xhtml" | "xml" | "svg") => plain(path).map(|text| markup_text(&text)),
        (FileCategory::Text, _) => plain(path),
        _ => return Ok(None),
    }?;
    let text = clean(&text);
    Ok((!text.is_empty()).then_some(text))
}

/// Whitespace collapsed per line, empty lines dropped, capped.
fn clean(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        if out.len() + line.len() > MAX_TEXT_CHARS {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.trim_end().to_owned()
}

fn plain(path: &Path) -> Result<String, VfsError> {
    let file = std::fs::File::open(path).map_err(|e| io(&e))?;
    let mut bytes = Vec::new();
    file.take(MAX_TEXT_BYTES).read_to_end(&mut bytes).map_err(|e| io(&e))?;
    if bytes.contains(&0) && !bytes.starts_with(&[0xFF, 0xFE]) && !bytes.starts_with(&[0xFE, 0xFF]) {
        // Binary content with a text-like extension (e.g. a lock file).
        return Ok(String::new());
    }
    Ok(decode(&bytes))
}

/// UTF-8, or UTF-16 with a byte-order mark.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |pairs: &[u8], little: bool| -> String {
        let units: Vec<u16> = pairs
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| if little { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn pdf(path: &Path) -> Result<String, VfsError> {
    let document = pdf_extract::Document::load(path).map_err(|e| VfsError::Io { message: e.to_string() })?;
    let mut text = String::new();
    let pages: Vec<u32> =
        document.get_pages().keys().copied().take(usize::try_from(MAX_PDF_PAGES).unwrap_or(40)).collect();
    for page in pages {
        let mut output = pdf_extract::PlainTextOutput::new(&mut text);
        // A page that fails (odd fonts, broken streams) is skipped, not fatal.
        let _ = pdf_extract::output_doc_page(&document, &mut output, page);
        text.push('\n');
        if text.len() > MAX_TEXT_CHARS {
            break;
        }
    }
    Ok(text)
}

/// The text of the zip members `wanted` picks, in name order (slides,
/// EPUB chapters).
fn zipped(path: &Path, wanted: impl Fn(&str) -> bool) -> Result<String, VfsError> {
    let file = std::fs::File::open(path).map_err(|e| io(&e))?;
    let mut archive =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| VfsError::Io { message: e.to_string() })?;
    let mut names: Vec<String> =
        archive.file_names().filter_map(Result::ok).filter(|n| wanted(n)).map(std::borrow::Cow::into_owned).collect();
    names.sort_by_key(|name| natural_key(name));
    let mut text = String::new();
    for name in names {
        let xml = member(&mut archive, &name)?;
        text.push_str(&markup_text(&xml));
        text.push('\n');
        if text.len() > MAX_TEXT_CHARS {
            break;
        }
    }
    Ok(text)
}

/// `slide10.xml` sorts after `slide9.xml`.
fn natural_key(name: &str) -> (String, u64) {
    let digits: String = name.chars().filter(char::is_ascii_digit).collect();
    let stem: String = name.chars().filter(|c| !c.is_ascii_digit()).collect();
    (stem, digits.parse().unwrap_or(0))
}

fn member<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Result<String, VfsError> {
    let entry = archive.by_name(name).map_err(|e| VfsError::Io { message: e.to_string() })?;
    let mut bytes = Vec::new();
    entry.take(MAX_MEMBER_BYTES).read_to_end(&mut bytes).map_err(|e| io(&e))?;
    Ok(decode(&bytes))
}

/// Text nodes of XML/HTML, with line breaks after block elements. Tolerant:
/// HTML that isn't well-formed XML still yields what was read so far.
fn markup_text(markup: &str) -> String {
    let mut reader = Reader::from_str(markup);
    let config = reader.config_mut();
    config.check_end_names = false;
    config.trim_text(false);
    let mut text = String::new();
    let mut skipping = 0_usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                if SKIPPED_ELEMENTS.contains(&local(element.local_name().as_ref())) {
                    skipping += 1;
                }
            }
            Ok(Event::End(element)) => {
                let name = local(element.local_name().as_ref()).to_owned();
                if SKIPPED_ELEMENTS.contains(&name.as_str()) {
                    skipping = skipping.saturating_sub(1);
                } else if BLOCK_ELEMENTS.contains(&name.as_str()) {
                    text.push('\n');
                }
            }
            Ok(Event::Empty(element)) => {
                if BLOCK_ELEMENTS.contains(&local(element.local_name().as_ref())) {
                    text.push('\n');
                }
            }
            Ok(Event::Text(content)) if skipping == 0 => {
                text.push_str(&content.into_inner());
                text.push(' ');
            }
            Ok(Event::CData(content)) if skipping == 0 => text.push_str(&content.into_inner()),
            Ok(Event::GeneralRef(reference)) if skipping == 0 => {
                let raw = format!("&{};", reference.into_inner());
                text.push_str(&quick_xml::escape::unescape(&raw).map(|s| s.into_owned()).unwrap_or_default());
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        if text.len() > MAX_TEXT_CHARS {
            break;
        }
    }
    text
}

/// `w:p` → `p`.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// RTF without groups like `{\fonttbl …}` and without `\control` words.
fn rtf_text(rtf: &str) -> String {
    let mut text = String::new();
    let mut chars = rtf.chars().peekable();
    let mut ignored_depth: Option<usize> = None;
    let mut depth = 0_usize;
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                depth += 1;
                if chars.peek() == Some(&'\\') && ignored_depth.is_none() {
                    let lookahead: String = chars.clone().take(12).collect();
                    let destination = ["\\*", "\\fonttbl", "\\colortbl", "\\stylesheet", "\\info", "\\pict"];
                    if destination.iter().any(|d| lookahead.starts_with(d)) {
                        ignored_depth = Some(depth);
                    }
                }
            }
            '}' => {
                if ignored_depth == Some(depth) {
                    ignored_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            '\\' => {
                let word: String = std::iter::from_fn(|| chars.next_if(char::is_ascii_alphabetic)).collect();
                while chars.next_if(|c| c.is_ascii_digit() || *c == '-').is_some() {}
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                if ignored_depth.is_none() && (word == "par" || word == "line") {
                    text.push('\n');
                }
                if word.is_empty()
                    && let Some(escaped) = chars.next().filter(|c| ['\\', '{', '}'].contains(c))
                {
                    text.push(escaped);
                }
            }
            c if ignored_depth.is_none() && c != '\r' && c != '\n' => text.push(c),
            _ => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_keeps_text_and_breaks_blocks() {
        let html = "<html><head><title>T</title><style>p{}</style></head><body><p>Faktura &amp; kvittering</p><script>x()</script><p>nr 42</p></body></html>";
        assert_eq!(clean(&markup_text(html)), "Faktura & kvittering\nnr 42");
    }

    #[test]
    fn rtf_loses_its_controls() {
        let rtf = r"{\rtf1\ansi{\fonttbl\f0 Helvetica;}\f0\fs24 Hello \b world\b0\par Second line}";
        assert_eq!(clean(&rtf_text(rtf)), "Hello world\nSecond line");
    }

    #[test]
    fn utf16_with_a_bom_is_decoded() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("Hej".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode(&bytes), "Hej");
    }
}
