//! Checksums: the algorithms offered, digests as text, and checking a digest
//! someone published (pasted, or in a `SHA256SUMS`-style file) against ours.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HashAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Blake3,
}

impl HashAlgorithm {
    pub const ALL: [Self; 4] = [Self::Md5, Self::Sha1, Self::Sha256, Self::Blake3];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Sha1 => "SHA-1",
            Self::Sha256 => "SHA-256",
            Self::Blake3 => "BLAKE3",
        }
    }

    /// Length of the digest in hex characters.
    #[must_use]
    pub const fn hex_len(self) -> usize {
        match self {
            Self::Md5 => 32,
            Self::Sha1 => 40,
            Self::Sha256 | Self::Blake3 => 64,
        }
    }

    /// MD5 and SHA-1 are broken for adversarial use: fine to catch a damaged
    /// download, not to prove a file wasn't tampered with.
    #[must_use]
    pub const fn is_collision_resistant(self) -> bool {
        matches!(self, Self::Sha256 | Self::Blake3)
    }
}

/// A digest as lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Digest {
    algorithm: HashAlgorithm,
    hex: String,
}

impl Digest {
    #[must_use]
    pub fn from_bytes(algorithm: HashAlgorithm, bytes: &[u8]) -> Self {
        use fmt::Write;
        let mut hex = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ = write!(hex, "{byte:02x}");
        }
        Self { algorithm, hex }
    }

    #[must_use]
    pub fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    #[must_use]
    pub fn hex(&self) -> &str {
        &self.hex
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex)
    }
}

/// How a published digest compares with the ones computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    Matches(HashAlgorithm),
    /// A valid digest of a computed length, but no computed digest equals it.
    Differs,
    /// Not hex, or no algorithm has this length.
    NotADigest,
}

/// Compares `published` (any case, surrounding spaces, an optional
/// `algo:` prefix) with the computed digests.
#[must_use]
pub fn verify(published: &str, computed: &[Digest]) -> Verification {
    let Some(hex) = normalised_hex(published) else { return Verification::NotADigest };
    let candidates: Vec<&Digest> = computed.iter().filter(|d| d.algorithm.hex_len() == hex.len()).collect();
    if candidates.is_empty() {
        return Verification::NotADigest;
    }
    candidates.iter().find(|d| d.hex == hex).map_or(Verification::Differs, |d| Verification::Matches(d.algorithm))
}

fn normalised_hex(text: &str) -> Option<String> {
    let text = text.trim();
    let text = text.split_once(':').map_or(text, |(_, rest)| rest).trim();
    let valid = !text.is_empty() && text.chars().all(|c| c.is_ascii_hexdigit());
    valid.then(|| text.to_ascii_lowercase())
}

/// Finds the digest for `file_name` in a checksum list, in GNU
/// (`<hex>  name`, `<hex> *name`) or BSD (`SHA256 (name) = <hex>`) form.
#[must_use]
pub fn digest_in_list(list: &str, file_name: &str) -> Option<String> {
    list.lines().find_map(|line| {
        let line = line.trim();
        if let Some((label, hex)) = line.rsplit_once(" = ") {
            let name = label.split_once(" (").and_then(|(_, rest)| rest.strip_suffix(')'))?;
            return (name == file_name).then(|| hex.trim().to_owned());
        }
        let (hex, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start();
        let name = name.strip_prefix('*').unwrap_or(name);
        (name == file_name || name.rsplit('/').next() == Some(file_name)).then(|| hex.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computed() -> Vec<Digest> {
        vec![
            Digest::from_bytes(HashAlgorithm::Md5, &[0xab; 16]),
            Digest::from_bytes(HashAlgorithm::Sha256, &[0x01; 32]),
            Digest::from_bytes(HashAlgorithm::Blake3, &[0x02; 32]),
        ]
    }

    #[test]
    fn digests_are_lowercase_hex() {
        assert_eq!(Digest::from_bytes(HashAlgorithm::Md5, &[0, 15, 255]).hex(), "000fff");
    }

    #[test]
    fn verification_picks_the_algorithm_by_length_and_value() {
        let digests = computed();
        assert_eq!(verify(&"AB".repeat(16), &digests), Verification::Matches(HashAlgorithm::Md5));
        assert_eq!(
            verify(&format!("  sha256:{} ", "01".repeat(32)), &digests),
            Verification::Matches(HashAlgorithm::Sha256)
        );
        assert_eq!(verify(&"02".repeat(32), &digests), Verification::Matches(HashAlgorithm::Blake3));
        assert_eq!(verify(&"03".repeat(32), &digests), Verification::Differs);
        assert_eq!(verify(&"ab".repeat(20), &digests), Verification::NotADigest, "SHA-1 wasn't computed");
        assert_eq!(verify("not a hash", &digests), Verification::NotADigest);
    }

    #[test]
    fn checksum_lists_in_gnu_and_bsd_form() {
        let gnu = "aaaa  other.iso\nbbbb *ubuntu.iso\n";
        assert_eq!(digest_in_list(gnu, "ubuntu.iso").as_deref(), Some("bbbb"));
        assert_eq!(digest_in_list("cccc  dist/app.dmg", "app.dmg").as_deref(), Some("cccc"));
        let bsd = "SHA256 (other) = 1111\nSHA256 (app.tar.gz) = 2222";
        assert_eq!(digest_in_list(bsd, "app.tar.gz").as_deref(), Some("2222"));
        assert_eq!(digest_in_list(gnu, "missing"), None);
    }
}
