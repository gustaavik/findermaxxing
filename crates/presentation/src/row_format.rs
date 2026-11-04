use domain::{ByteSize, EntryKind, FileEntry, PathSegment, StoragePath, SymlinkTarget};

use crate::rows_bridge::EntryRow;

/// Decimal units, matching Finder and GNOME Files.
const UNIT_STEP: f64 = 1000.0;
const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];

/// Icon tokens understood by `FileIcon.qml`.
mod icon {
    pub const FOLDER: &str = "folder";
    pub const FILE: &str = "file";
    pub const LINK: &str = "link";
    pub const BROKEN_LINK: &str = "broken-link";
    pub const OTHER: &str = "other";
}

/// Shown in the folder column for results directly inside the search scope.
const SCOPE_FOLDER_TEXT: &str = "—";
const FOLDER_SEPARATOR: &str = "/";

/// How rows describe their entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowContext {
    Browse,
    /// Search results also say which folder (below `scope`) they are in.
    SearchIn(StoragePath),
}

impl RowContext {
    pub(crate) fn row(&self, entry: &FileEntry) -> EntryRow {
        let mut row = to_row(entry);
        if let Self::SearchIn(scope) = self {
            row.folder_text = folder_text(entry, scope);
        }
        row
    }
}

fn folder_text(entry: &FileEntry, scope: &StoragePath) -> String {
    let Some(parent) = entry.path().parent() else { return String::new() };
    let below = parent.segments().strip_prefix(scope.segments()).unwrap_or(parent.segments());
    if below.is_empty() {
        return SCOPE_FOLDER_TEXT.to_owned();
    }
    below.iter().map(PathSegment::display_name).collect::<Vec<_>>().join(FOLDER_SEPARATOR)
}

pub(crate) fn to_row(entry: &FileEntry) -> EntryRow {
    let modified = entry.metadata().modified;
    EntryRow {
        name: entry.name().to_string(),
        location: entry.path().encode(),
        icon_name: icon_name(entry.kind()).to_owned(),
        size_text: entry.metadata().size.map(format_size).unwrap_or_default(),
        modified_ms: modified.map_or(0, domain::Timestamp::unix_millis),
        has_modified: modified.is_some(),
        is_container: entry.kind().is_container(),
        is_hidden: entry.is_hidden(),
        folder_text: String::new(),
    }
}

fn icon_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Directory | EntryKind::Symlink(SymlinkTarget::Directory) => icon::FOLDER,
        EntryKind::File => icon::FILE,
        EntryKind::Symlink(SymlinkTarget::File) => icon::LINK,
        EntryKind::Symlink(SymlinkTarget::Broken) => icon::BROKEN_LINK,
        EntryKind::Other => icon::OTHER,
    }
}

#[allow(clippy::cast_precision_loss, reason = "display rounding to one decimal")]
pub(crate) fn format_size(size: ByteSize) -> String {
    let bytes = size.0;
    if (bytes as f64) < UNIT_STEP {
        return if bytes == 1 { "1 byte".to_owned() } else { format!("{bytes} bytes") };
    }
    let mut value = bytes as f64 / UNIT_STEP;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if value < UNIT_STEP {
            break;
        }
        value /= UNIT_STEP;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(format_size(ByteSize(0)), "0 bytes");
        assert_eq!(format_size(ByteSize(1)), "1 byte");
        assert_eq!(format_size(ByteSize(999)), "999 bytes");
        assert_eq!(format_size(ByteSize(1_000)), "1.0 KB");
        assert_eq!(format_size(ByteSize(1_536_000)), "1.5 MB");
        assert_eq!(format_size(ByteSize(3_000_000_000)), "3.0 GB");
    }
}
