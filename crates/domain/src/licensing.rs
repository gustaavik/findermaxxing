//! Editions and licenses (ADR 0030): a free core, and Pro features unlocked
//! by a license activated on this machine. The rules work offline: a signed
//! machine file proves the activation until it expires, and is renewed in
//! the background while online.

use crate::units::Timestamp;

const MILLIS_PER_DAY: i64 = 86_400_000;
/// Renew the machine file when it has less than this left.
pub const RENEW_BEFORE_DAYS: i64 = 10;

/// What needs Pro. Everything else (local browsing, file operations,
/// previews, search, bulk rename) is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProFeature {
    /// S3, file servers and cloud drives.
    RemoteStorage,
    /// The content index and searching by meaning.
    AiSearch,
    /// Organization suggestions.
    Organize,
    /// Archives, checksums, duplicates, disk usage, image tools,
    /// permissions, secure delete and the terminal.
    Utilities,
}

impl ProFeature {
    pub const ALL: [Self; 4] = [Self::RemoteStorage, Self::AiSearch, Self::Organize, Self::Utilities];

    /// Stable names, used by the UI.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::RemoteStorage => "remote-storage",
            Self::AiSearch => "ai-search",
            Self::Organize => "organize",
            Self::Utilities => "utilities",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.key() == key)
    }
}

/// A verified activation of this machine, as read from a machine file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    pub license_id: String,
    /// The licensee or license name, shown in the app.
    pub licensee: String,
    pub fingerprint: String,
    /// When the license itself ends; `None` for perpetual licenses.
    pub license_expiry: Option<Timestamp>,
    /// Suspended or banned on the server when the file was issued.
    pub license_active: bool,
    pub issued: Timestamp,
    /// When the machine file stops proving the activation (offline limit).
    pub file_expiry: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseStatus {
    /// No license on this machine.
    Free,
    Pro {
        licensee: String,
        license_expiry: Option<Timestamp>,
    },
    /// The license ended (or was suspended); Pro features are off.
    Ended {
        licensee: String,
    },
    /// Activated, but not confirmed for too long (offline); reconnect.
    NeedsConfirmation {
        licensee: String,
    },
    /// The activation belongs to another machine (copied settings).
    OtherMachine,
    /// The clock is before the file was issued: it was set back.
    ClockSetBack,
}

impl LicenseStatus {
    #[must_use]
    pub fn is_pro(&self) -> bool {
        matches!(self, Self::Pro { .. })
    }

    #[must_use]
    pub fn allows(&self, _feature: ProFeature) -> bool {
        self.is_pro()
    }
}

/// The status of `activation` (if any) on the machine `fingerprint` at `now`.
#[must_use]
pub fn license_status(activation: Option<&Activation>, fingerprint: &str, now: Timestamp) -> LicenseStatus {
    let Some(activation) = activation else { return LicenseStatus::Free };
    if activation.fingerprint != fingerprint {
        return LicenseStatus::OtherMachine;
    }
    if now < activation.issued {
        return LicenseStatus::ClockSetBack;
    }
    let licensee = activation.licensee.clone();
    if !activation.license_active || activation.license_expiry.is_some_and(|end| end < now) {
        return LicenseStatus::Ended { licensee };
    }
    if activation.file_expiry.is_some_and(|end| end < now) {
        return LicenseStatus::NeedsConfirmation { licensee };
    }
    LicenseStatus::Pro { licensee, license_expiry: activation.license_expiry }
}

/// Whether the machine file should be renewed now (when online).
#[must_use]
pub fn should_renew(activation: &Activation, now: Timestamp) -> bool {
    activation.file_expiry.is_some_and(|end| end.unix_millis() - now.unix_millis() < RENEW_BEFORE_DAYS * MILLIS_PER_DAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = Timestamp::from_unix_millis(1_793_793_600_000);

    fn days(n: i64) -> Timestamp {
        Timestamp::from_unix_millis(NOW.unix_millis() + n * MILLIS_PER_DAY)
    }

    fn activation() -> Activation {
        Activation {
            license_id: "lic".into(),
            licensee: "Ada".into(),
            fingerprint: "fp".into(),
            license_expiry: None,
            license_active: true,
            issued: days(-1),
            file_expiry: Some(days(29)),
        }
    }

    #[test]
    fn a_fresh_activation_is_pro_on_its_own_machine_only() {
        assert!(license_status(Some(&activation()), "fp", NOW).is_pro());
        assert_eq!(license_status(Some(&activation()), "other", NOW), LicenseStatus::OtherMachine);
        assert_eq!(license_status(None, "fp", NOW), LicenseStatus::Free);
        assert!(!LicenseStatus::Free.allows(ProFeature::AiSearch));
    }

    #[test]
    fn ended_licenses_offline_limits_and_set_back_clocks() {
        let ended = Activation { license_expiry: Some(days(-2)), ..activation() };
        assert_eq!(license_status(Some(&ended), "fp", NOW), LicenseStatus::Ended { licensee: "Ada".into() });
        let suspended = Activation { license_active: false, ..activation() };
        assert!(matches!(license_status(Some(&suspended), "fp", NOW), LicenseStatus::Ended { .. }));
        let stale = Activation { file_expiry: Some(days(-1)), ..activation() };
        assert!(matches!(license_status(Some(&stale), "fp", NOW), LicenseStatus::NeedsConfirmation { .. }));
        let future = Activation { issued: days(3), ..activation() };
        assert_eq!(license_status(Some(&future), "fp", NOW), LicenseStatus::ClockSetBack);
    }

    #[test]
    fn files_are_renewed_well_before_they_expire() {
        assert!(!should_renew(&activation(), NOW));
        assert!(should_renew(&Activation { file_expiry: Some(days(5)), ..activation() }, NOW));
        assert_eq!(ProFeature::from_key("organize"), Some(ProFeature::Organize));
    }
}
