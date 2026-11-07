//! Archive formats by name, the names of what compressing and extracting
//! create, and the safety rule for paths inside archives.

use crate::error::PathError;
use crate::path::{PathSegment, StoragePath};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    SevenZ,
    /// Read-only: RAR's licence forbids creating archives.
    Rar,
}

/// Suffixes, longest first so `.tar.gz` wins over `.gz`.
const SUFFIXES: &[(&str, ArchiveFormat)] = &[
    (".tar.gz", ArchiveFormat::TarGz),
    (".tar.bz2", ArchiveFormat::TarBz2),
    (".tar.xz", ArchiveFormat::TarXz),
    (".tar.zst", ArchiveFormat::TarZst),
    (".tgz", ArchiveFormat::TarGz),
    (".tbz2", ArchiveFormat::TarBz2),
    (".tbz", ArchiveFormat::TarBz2),
    (".txz", ArchiveFormat::TarXz),
    (".tzst", ArchiveFormat::TarZst),
    (".tar", ArchiveFormat::Tar),
    (".zip", ArchiveFormat::Zip),
    (".7z", ArchiveFormat::SevenZ),
    (".rar", ArchiveFormat::Rar),
];

/// The formats offered when compressing, in menu order.
pub const CREATABLE_FORMATS: [ArchiveFormat; 7] = [
    ArchiveFormat::Zip,
    ArchiveFormat::SevenZ,
    ArchiveFormat::TarGz,
    ArchiveFormat::TarXz,
    ArchiveFormat::TarZst,
    ArchiveFormat::TarBz2,
    ArchiveFormat::Tar,
];

/// Base name of a new archive of several items.
const SEVERAL_ITEMS_NAME: &str = "Archive";

impl ArchiveFormat {
    #[must_use]
    pub fn of(name: &PathSegment) -> Option<Self> {
        let lower = name.display_name().to_lowercase();
        SUFFIXES.iter().find(|(suffix, _)| lower.ends_with(suffix)).map(|(_, format)| *format)
    }

    #[must_use]
    pub const fn can_create(self) -> bool {
        !matches!(self, Self::Rar)
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Zip => ".zip",
            Self::Tar => ".tar",
            Self::TarGz => ".tar.gz",
            Self::TarBz2 => ".tar.bz2",
            Self::TarXz => ".tar.xz",
            Self::TarZst => ".tar.zst",
            Self::SevenZ => ".7z",
            Self::Rar => ".rar",
        }
    }
}

/// The folder an archive extracts into: its name without the archive suffix
/// (`photos.tar.gz` → `photos`).
///
/// # Errors
/// Never in practice; falls back to the full name if the stem is empty.
pub fn extraction_folder_name(archive: &PathSegment) -> Result<PathSegment, PathError> {
    let name = archive.display_name();
    let stem = ArchiveFormat::of(archive)
        .and_then(|format| {
            let suffix =
                SUFFIXES.iter().filter(|(_, f)| *f == format).find(|(s, _)| name.to_lowercase().ends_with(s))?.0;
            name.get(..name.len() - suffix.len())
        })
        .filter(|stem| !stem.is_empty())
        .unwrap_or(&name);
    PathSegment::new(stem)
}

/// The name of a new archive: the item's own name for one item
/// (`Report.pdf.zip`, `Photos.zip`), `Archive.zip` for several.
///
/// # Errors
/// Never in practice; names are built from valid segments.
pub fn archive_name(items: &[StoragePath], format: ArchiveFormat) -> Result<PathSegment, PathError> {
    let base = match items {
        [single] => single.name().map_or_else(|| SEVERAL_ITEMS_NAME.to_owned(), |n| n.display_name().into_owned()),
        _ => SEVERAL_ITEMS_NAME.to_owned(),
    };
    PathSegment::new(&format!("{base}{}", format.extension()))
}

/// Why an entry inside an archive is not extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafeEntry {
    /// `..` would escape the destination ("zip slip").
    EscapesFolder,
    /// Nothing left after removing `/`, `.` and empty parts.
    Empty,
    /// A part isn't a valid name.
    Invalid,
}

/// Path of an archive entry, relative to the destination folder. Leading
/// slashes and drive letters are dropped (as `tar` and `unzip` do); `..`
/// anywhere refuses the entry. Both separators are accepted, since zips
/// made on Windows sometimes use `\`.
///
/// # Errors
/// [`UnsafeEntry`] when the entry must not be extracted.
pub fn entry_path(raw: &[u8]) -> Result<Vec<PathSegment>, UnsafeEntry> {
    let raw = match raw {
        [letter, b':', rest @ ..] if letter.is_ascii_alphabetic() => rest,
        other => other,
    };
    let mut segments = Vec::new();
    for part in raw.split(|b| *b == b'/' || *b == b'\\') {
        match part {
            b"" | b"." => {}
            b".." => return Err(UnsafeEntry::EscapesFolder),
            part => segments.push(PathSegment::from_bytes(part).map_err(|_| UnsafeEntry::Invalid)?),
        }
    }
    if segments.is_empty() { Err(UnsafeEntry::Empty) } else { Ok(segments) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(name: &str) -> PathSegment {
        PathSegment::new(name).unwrap()
    }

    #[test]
    fn formats_are_recognised_by_suffix() {
        assert_eq!(ArchiveFormat::of(&seg("a.TAR.GZ")), Some(ArchiveFormat::TarGz));
        assert_eq!(ArchiveFormat::of(&seg("a.tgz")), Some(ArchiveFormat::TarGz));
        assert_eq!(ArchiveFormat::of(&seg("a.7z")), Some(ArchiveFormat::SevenZ));
        assert_eq!(ArchiveFormat::of(&seg("notes.gz")), None, "a lone .gz is not an archive of files");
        assert!(!ArchiveFormat::Rar.can_create());
    }

    #[test]
    fn extraction_folders_drop_the_whole_suffix() {
        assert_eq!(extraction_folder_name(&seg("photos.tar.gz")).unwrap(), seg("photos"));
        assert_eq!(extraction_folder_name(&seg("Report.v2.ZIP")).unwrap(), seg("Report.v2"));
        assert_eq!(extraction_folder_name(&seg(".zip")).unwrap(), seg(".zip"));
    }

    #[test]
    fn archive_names_follow_finder() {
        let p = |s: &str| StoragePath::parse(&format!("file://{s}")).unwrap();
        assert_eq!(archive_name(&[p("/d/Report.pdf")], ArchiveFormat::Zip).unwrap(), seg("Report.pdf.zip"));
        assert_eq!(archive_name(&[p("/d/a"), p("/d/b")], ArchiveFormat::TarXz).unwrap(), seg("Archive.tar.xz"));
    }

    #[test]
    fn entries_never_escape_the_destination() {
        assert_eq!(entry_path(b"dir/./file.txt").unwrap(), [seg("dir"), seg("file.txt")]);
        assert_eq!(entry_path(b"/etc/passwd").unwrap(), [seg("etc"), seg("passwd")]);
        assert_eq!(entry_path(b"C:\\Users\\x.txt").unwrap(), [seg("Users"), seg("x.txt")]);
        assert_eq!(entry_path(b"a/../../evil"), Err(UnsafeEntry::EscapesFolder));
        assert_eq!(entry_path(b"..\\evil"), Err(UnsafeEntry::EscapesFolder));
        assert_eq!(entry_path(b"/./"), Err(UnsafeEntry::Empty));
    }
}
