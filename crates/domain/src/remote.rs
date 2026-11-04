//! Remote storage accounts. S3 is the first; each provider brings its own
//! settings type, while mounts stay plain `MountId`s.

use std::fmt;

use crate::error::PathError;
use crate::path::{MountId, Scheme};

pub const S3_SCHEME: &str = "s3";
/// Separates account and bucket in an S3 mount authority (`3@photos`).
/// Never valid in a bucket name, so the split is unambiguous.
const ACCOUNT_SEPARATOR: char = '@';
const MIN_BUCKET_LENGTH: usize = 3;
const MAX_BUCKET_LENGTH: usize = 63;

/// Identifier assigned by the account store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub i64);

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An S3 bucket name following the (lenient, MinIO/R2-compatible) rules:
/// 3–63 characters of lowercase letters, digits, `.`, `-` and `_`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BucketName(String);

impl BucketName {
    /// # Errors
    /// [`PathError::InvalidAuthority`] for names S3 would reject.
    pub fn new(name: &str) -> Result<Self, PathError> {
        let length_ok = (MIN_BUCKET_LENGTH..=MAX_BUCKET_LENGTH).contains(&name.len());
        let chars_ok =
            name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
        if length_ok && chars_ok {
            Ok(Self(name.to_owned()))
        } else {
            Err(PathError::InvalidAuthority(name.to_owned()))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BucketName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which bucket of which account a `s3://` mount refers to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3Location {
    pub account: AccountId,
    pub bucket: BucketName,
}

impl S3Location {
    /// # Errors
    /// Never in practice; the scheme literal and authority are valid.
    pub fn mount(&self) -> Result<MountId, PathError> {
        MountId::new(Scheme::new(S3_SCHEME)?, &format!("{}{ACCOUNT_SEPARATOR}{}", self.account, self.bucket))
    }

    /// `None` for mounts that are not S3 buckets.
    #[must_use]
    pub fn of(mount: &MountId) -> Option<Self> {
        if mount.scheme().as_str() != S3_SCHEME {
            return None;
        }
        let (account, bucket) = mount.authority().split_once(ACCOUNT_SEPARATOR)?;
        Some(Self { account: AccountId(account.parse().ok()?), bucket: BucketName::new(bucket).ok()? })
    }
}

/// How the app authenticates to an S3 account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3Auth {
    /// The AWS default chain: environment, `~/.aws`, SSO, instance roles.
    DefaultChain,
    /// A named profile from `~/.aws/config` (including SSO profiles).
    Profile(String),
    /// An access key; the secret lives in the OS keychain, never here.
    AccessKey { access_key_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Addressing {
    /// `bucket.host` (AWS, R2, B2, Wasabi).
    #[default]
    VirtualHosted,
    /// `host/bucket` (MinIO and most self-hosted servers).
    PathStyle,
}

/// Settings of an S3 or S3-compatible account. Contains no secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Account {
    pub id: AccountId,
    pub name: String,
    /// `None` means AWS; otherwise the service URL (MinIO, R2, B2, Wasabi…).
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub auth: S3Auth,
    pub addressing: Addressing,
    /// The user opted out of cost confirmations for this account.
    pub skip_cost_warnings: bool,
    /// Buckets added by name: scoped keys often can't list buckets at all.
    pub extra_buckets: Vec<BucketName>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_names_follow_s3_rules() {
        assert!(BucketName::new("my.photos-2024").is_ok());
        for bad in ["ab", "Upper", "has space", "a@b", &"x".repeat(64)] {
            assert!(BucketName::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn location_round_trips_through_mounts() {
        let location = S3Location { account: AccountId(7), bucket: BucketName::new("my.bucket").unwrap() };
        let mount = location.mount().unwrap();
        assert_eq!(mount.authority(), "7@my.bucket");
        assert_eq!(S3Location::of(&mount), Some(location));
        assert_eq!(S3Location::of(&MountId::new(Scheme::new("file").unwrap(), "").unwrap()), None);
    }
}
