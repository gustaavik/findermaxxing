//! What counts as clutter: old downloads, installers that have done their
//! job, big files nobody has touched in a long time, and screenshot piles.
//! Duplicates come from the duplicate finder.

use crate::entry::{EntryKind, FileEntry};
use crate::units::{ByteSize, Timestamp};

const MILLIS_PER_DAY: i64 = 86_400_000;

/// When something counts as clutter. Conservative: suggestions are cheap
/// to reject, but noisy ones teach people to ignore them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClutterPolicy {
    pub old_download_days: i64,
    pub old_installer_days: i64,
    pub large_file: ByteSize,
    pub large_unchanged_days: i64,
    /// Screenshots in one folder before they count as a pile.
    pub screenshot_pile: usize,
}

impl Default for ClutterPolicy {
    fn default() -> Self {
        Self {
            old_download_days: 90,
            old_installer_days: 14,
            large_file: ByteSize(500 * 1024 * 1024),
            large_unchanged_days: 365,
            screenshot_pile: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClutterReason {
    /// In Downloads and not changed for this many days.
    OldDownload { days: i64 },
    /// A disk image or installer package, probably installed already.
    Installer { days: i64 },
    /// Large and not changed for this many days.
    LargeAndUnchanged { size: ByteSize, days: i64 },
    /// Byte-for-byte the same as the named item, which is kept.
    DuplicateOf(String),
    /// Looks the same as the named image, which is kept.
    LooksLike(String),
}

const INSTALLER_EXTENSIONS: &[&str] = &["dmg", "pkg", "mpkg", "exe", "msi", "deb", "rpm", "appimage", "flatpakref"];
/// How macOS (several languages), GNOME and KDE name screenshots.
const SCREENSHOT_PREFIXES: &[&str] = &[
    "screenshot",
    "screen shot",
    "skærmbillede",
    "bildschirmfoto",
    "capture d’écran",
    "capture d'écran",
    "captura de pantalla",
    "schermafbeelding",
];
const SCREENSHOT_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "heic"];
/// Names of the downloads folder, in the languages above.
const DOWNLOAD_FOLDERS: &[&str] = &["downloads", "overførsler", "téléchargements", "descargas"];

fn lower_extension(entry: &FileEntry) -> Option<String> {
    entry.name().stem_and_extension().1.map(|e| String::from_utf8_lossy(e).to_lowercase())
}

#[must_use]
pub fn is_installer(entry: &FileEntry) -> bool {
    lower_extension(entry).is_some_and(|e| INSTALLER_EXTENSIONS.contains(&e.as_str()))
}

#[must_use]
pub fn is_screenshot(entry: &FileEntry) -> bool {
    let name = entry.name().display_name().to_lowercase();
    entry.kind() == EntryKind::File
        && SCREENSHOT_PREFIXES.iter().any(|p| name.starts_with(p))
        && lower_extension(entry).is_some_and(|e| SCREENSHOT_EXTENSIONS.contains(&e.as_str()))
}

/// Whether a folder name is the user's downloads folder.
#[must_use]
pub fn is_downloads_folder(name: &str) -> bool {
    DOWNLOAD_FOLDERS.contains(&name.to_lowercase().as_str())
}

/// Why `entry` is clutter, if it is. `in_downloads`: it sits in Downloads.
#[must_use]
pub fn clutter_of(
    entry: &FileEntry,
    in_downloads: bool,
    now: Timestamp,
    policy: &ClutterPolicy,
) -> Option<ClutterReason> {
    if entry.is_hidden() {
        return None;
    }
    let age_days =
        entry.metadata().modified.map(|m| (now.unix_millis() - m.unix_millis()).div_euclid(MILLIS_PER_DAY))?;
    let size = entry.metadata().size.unwrap_or_default();
    if entry.kind() == EntryKind::File && is_installer(entry) && age_days >= policy.old_installer_days {
        return Some(ClutterReason::Installer { days: age_days });
    }
    if entry.kind() == EntryKind::File && size >= policy.large_file && age_days >= policy.large_unchanged_days {
        return Some(ClutterReason::LargeAndUnchanged { size, days: age_days });
    }
    (in_downloads && age_days >= policy.old_download_days).then_some(ClutterReason::OldDownload { days: age_days })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryMetadata, PathSegment, StoragePath};

    const NOW: Timestamp = Timestamp::from_unix_millis(1_793_793_600_000);

    fn entry(name: &str, size: u64, days_old: i64) -> FileEntry {
        let metadata = EntryMetadata {
            size: Some(ByteSize(size)),
            modified: Some(Timestamp::from_unix_millis(NOW.unix_millis() - days_old * MILLIS_PER_DAY)),
            ..EntryMetadata::default()
        };
        FileEntry::new(
            &StoragePath::parse("file:///d").unwrap(),
            PathSegment::new(name).unwrap(),
            EntryKind::File,
            metadata,
        )
    }

    #[test]
    fn installers_old_downloads_and_big_untouched_files() {
        let policy = ClutterPolicy::default();
        assert_eq!(
            clutter_of(&entry("Zoom.dmg", 10, 30), false, NOW, &policy),
            Some(ClutterReason::Installer { days: 30 })
        );
        assert_eq!(clutter_of(&entry("Zoom.dmg", 10, 3), false, NOW, &policy), None, "just downloaded");
        assert_eq!(
            clutter_of(&entry("a.pdf", 10, 120), true, NOW, &policy),
            Some(ClutterReason::OldDownload { days: 120 })
        );
        assert_eq!(clutter_of(&entry("a.pdf", 10, 120), false, NOW, &policy), None, "only in Downloads");
        let big = clutter_of(&entry("film.mkv", 600 * 1024 * 1024, 400), false, NOW, &policy);
        assert!(matches!(big, Some(ClutterReason::LargeAndUnchanged { days: 400, .. })));
    }

    #[test]
    fn screenshots_by_name_in_several_languages() {
        assert!(is_screenshot(&entry("Screenshot 2026-03-14 at 15.09.26.png", 1, 1)));
        assert!(is_screenshot(&entry("Skærmbillede 2026-03-14 kl. 15.09.26.png", 1, 1)));
        assert!(!is_screenshot(&entry("Screenshot notes.txt", 1, 1)));
        assert!(is_downloads_folder("Downloads"));
    }
}
