//! Consumer cloud drives signed in with OAuth: Google Drive, Dropbox and
//! OneDrive. One account is one mount (`gdrive://3/…`) whose root is the
//! drive's root ("My Drive", the Dropbox folder, the OneDrive root).

use std::fmt;

use crate::entry::EntryKind;
use crate::error::PathError;
use crate::path::{MountId, PathSegment, Scheme};
use crate::remote::AccountId;

pub const GOOGLE_DRIVE_SCHEME: &str = "gdrive";
pub const DROPBOX_SCHEME: &str = "dropbox";
pub const ONEDRIVE_SCHEME: &str = "onedrive";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DriveProvider {
    GoogleDrive,
    Dropbox,
    OneDrive,
}

impl DriveProvider {
    pub const ALL: [Self; 3] = [Self::GoogleDrive, Self::Dropbox, Self::OneDrive];

    #[must_use]
    pub fn scheme_name(self) -> &'static str {
        match self {
            Self::GoogleDrive => GOOGLE_DRIVE_SCHEME,
            Self::Dropbox => DROPBOX_SCHEME,
            Self::OneDrive => ONEDRIVE_SCHEME,
        }
    }

    /// # Errors
    /// Never in practice; the scheme literals are valid.
    pub fn scheme(self) -> Result<Scheme, PathError> {
        Scheme::new(self.scheme_name())
    }

    #[must_use]
    pub fn of_scheme(scheme: &Scheme) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.scheme_name() == scheme.as_str())
    }
}

impl fmt::Display for DriveProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::GoogleDrive => "Google Drive",
            Self::Dropbox => "Dropbox",
            Self::OneDrive => "OneDrive",
        })
    }
}

/// A signed-in drive. The OAuth refresh token lives in the OS keychain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveAccount {
    pub id: AccountId,
    pub provider: DriveProvider,
    /// What the sidebar shows; the provider's display name by default.
    pub name: String,
    /// The signed-in user, to tell two accounts of one provider apart.
    pub email: String,
}

impl DriveAccount {
    /// # Errors
    /// Never in practice; the scheme literal and numeric authority are valid.
    pub fn mount(&self) -> Result<MountId, PathError> {
        DriveLocation { provider: self.provider, account: self.id }.mount()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DriveLocation {
    pub provider: DriveProvider,
    pub account: AccountId,
}

impl DriveLocation {
    /// # Errors
    /// Never in practice; the scheme literal and numeric authority are valid.
    pub fn mount(&self) -> Result<MountId, PathError> {
        MountId::new(self.provider.scheme()?, &self.account.to_string())
    }

    /// `None` for mounts that are not drive accounts.
    #[must_use]
    pub fn of(mount: &MountId) -> Option<Self> {
        let provider = DriveProvider::of_scheme(mount.scheme())?;
        Some(Self { provider, account: AccountId(mount.authority().parse().ok()?) })
    }
}

/// Names for the items of one folder on a provider that allows several items
/// with the same name (Google Drive). `items` are in a stable order (oldest
/// first); the first of each name keeps it and later ones become
/// `report (2).pdf`, `report (3).pdf`, … skipping names already used by
/// another item. Nothing is renamed on the provider; only the app shows
/// these names, so every item stays reachable and addressable.
#[must_use]
pub fn disambiguate(items: &[(PathSegment, EntryKind)]) -> Vec<PathSegment> {
    let taken: std::collections::HashSet<&[u8]> = items.iter().map(|(name, _)| name.as_bytes()).collect();
    let mut used: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    items
        .iter()
        .map(|(name, kind)| {
            if used.insert(name.as_bytes().to_vec()) {
                return name.clone();
            }
            let mut number = FIRST_DISAMBIGUATION;
            loop {
                let candidate = numbered(name, *kind, number);
                let free = !taken.contains(candidate.as_bytes()) && !used.contains(candidate.as_bytes());
                if free {
                    used.insert(candidate.as_bytes().to_vec());
                    return candidate;
                }
                number += 1;
            }
        })
        .collect()
}

const FIRST_DISAMBIGUATION: u32 = 2;

/// `report.pdf` → `report (2).pdf`; folders and dot-files get the number at the end.
fn numbered(name: &PathSegment, kind: EntryKind, number: u32) -> PathSegment {
    let bytes = name.as_bytes();
    let dot = (kind != EntryKind::Directory).then(|| bytes.iter().rposition(|b| *b == b'.')).flatten();
    let (stem, extension) = match dot {
        Some(index) if index > 0 => bytes.split_at(index),
        _ => (bytes, &[][..]),
    };
    let mut numbered = stem.to_vec();
    numbered.extend_from_slice(format!(" ({number})").as_bytes());
    numbered.extend_from_slice(extension);
    PathSegment::from_bytes(&numbered).unwrap_or_else(|_| name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[(&str, EntryKind)]) -> Vec<String> {
        let segments: Vec<(PathSegment, EntryKind)> =
            items.iter().map(|(name, kind)| (PathSegment::new(name).unwrap(), *kind)).collect();
        disambiguate(&segments).iter().map(|s| s.display_name().into_owned()).collect()
    }

    #[test]
    fn later_duplicates_get_numbers_that_no_other_item_uses() {
        let file = EntryKind::File;
        assert_eq!(
            names(&[("report.pdf", file), ("report.pdf", file), ("report (2).pdf", file), ("report.pdf", file)]),
            ["report.pdf", "report (3).pdf", "report (2).pdf", "report (4).pdf"]
        );
        assert_eq!(
            names(&[("Photos", EntryKind::Directory), ("Photos", EntryKind::Directory)]),
            ["Photos", "Photos (2)"]
        );
        assert_eq!(names(&[(".env", file), (".env", file)]), [".env", ".env (2)"]);
    }

    #[test]
    fn locations_round_trip_through_mounts() {
        for provider in DriveProvider::ALL {
            let location = DriveLocation { provider, account: AccountId(7) };
            assert_eq!(DriveLocation::of(&location.mount().unwrap()), Some(location));
        }
    }
}
