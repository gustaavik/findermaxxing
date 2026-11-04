use crate::entry::{EntryKind, FileEntry};

/// What kind of thing an item is, for previews and the "Kind" search filter.
/// Decided by extension: cheap, deterministic, and identical on every provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileCategory {
    Folder,
    Image,
    Pdf,
    Video,
    Audio,
    Text,
    Archive,
    Document,
    Other,
}

const IMAGE: &[&str] =
    &["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "heic", "heif", "svg", "ico", "avif"];
const VIDEO: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "mpg", "mpeg", "3gp"];
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "oga", "opus", "aif", "aiff", "wma"];
const ARCHIVE: &[&str] = &["zip", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "7z", "rar", "jar"];
const DOCUMENT: &[&str] =
    &["doc", "docx", "odt", "rtf", "pages", "xls", "xlsx", "ods", "numbers", "ppt", "pptx", "odp", "key", "epub"];
// Kept compact on purpose: one extension per line would be 70 lines of noise.
#[rustfmt::skip]
const TEXT: &[&str] = &[
    "txt", "md", "markdown", "rst", "log", "csv", "tsv", "json", "jsonc", "yaml", "yml", "toml", "ini", "cfg",
    "conf", "xml", "html", "htm", "css", "scss", "js", "mjs", "cjs", "ts", "tsx", "jsx", "rs", "c", "h", "cc",
    "cpp", "hpp", "cxx", "m", "mm", "swift", "kt", "java", "go", "py", "rb", "php", "pl", "lua", "sh", "bash",
    "zsh", "fish", "ps1", "sql", "qml", "cmake", "gradle", "dart", "scala", "hs", "ex", "exs", "erl", "clj", "lisp",
    "el", "vim", "tex", "diff", "patch", "env", "gitignore", "dockerfile", "makefile", "lock",
];
/// Names that are text files without an extension.
const TEXT_NAMES: &[&str] = &["makefile", "dockerfile", "readme", "license", "copying", "changelog", "authors"];

impl FileCategory {
    #[must_use]
    pub fn of(entry: &FileEntry) -> Self {
        if entry.kind().is_container() {
            return Self::Folder;
        }
        if !matches!(entry.kind(), EntryKind::File | EntryKind::Symlink(_)) {
            return Self::Other;
        }
        let name = entry.name().display_name().to_lowercase();
        let extension = entry.name().stem_and_extension().1.map(|e| String::from_utf8_lossy(e).to_lowercase());
        match extension {
            Some(extension) => Self::from_extension(&extension),
            None if TEXT_NAMES.contains(&name.as_str()) => Self::Text,
            None => Self::Other,
        }
    }

    fn from_extension(extension: &str) -> Self {
        let table: [(&[&str], Self); 6] = [
            (IMAGE, Self::Image),
            (VIDEO, Self::Video),
            (AUDIO, Self::Audio),
            (ARCHIVE, Self::Archive),
            (DOCUMENT, Self::Document),
            (TEXT, Self::Text),
        ];
        if extension == "pdf" {
            return Self::Pdf;
        }
        table.iter().find(|(list, _)| list.contains(&extension)).map_or(Self::Other, |(_, category)| *category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryMetadata, PathSegment, StoragePath};

    fn category(name: &str, kind: EntryKind) -> FileCategory {
        let parent = StoragePath::parse("file:///d").unwrap();
        FileCategory::of(&FileEntry::new(&parent, PathSegment::new(name).unwrap(), kind, EntryMetadata::default()))
    }

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(category("Photo.JPG", EntryKind::File), FileCategory::Image);
        assert_eq!(category("paper.pdf", EntryKind::File), FileCategory::Pdf);
        assert_eq!(category("main.rs", EntryKind::File), FileCategory::Text);
        assert_eq!(category("backup.tar.gz", EntryKind::File), FileCategory::Archive);
        assert_eq!(category("Makefile", EntryKind::File), FileCategory::Text);
        assert_eq!(category("binary", EntryKind::File), FileCategory::Other);
        assert_eq!(category("photos.jpg", EntryKind::Directory), FileCategory::Folder);
    }
}
