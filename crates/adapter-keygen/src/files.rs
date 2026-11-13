//! Verifying and reading Keygen machine files offline.

use application::ports::{LicenseError, MachineFile, MachineFileReader};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use domain::{Activation, Timestamp};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use infra_oauth::parse_rfc3339;
use serde_json::Value;

const HEADER: &str = "-----BEGIN MACHINE FILE-----";
const FOOTER: &str = "-----END MACHINE FILE-----";
const ALGORITHM: &str = "base64+ed25519";
/// What Keygen signs: the file type, a slash, then `enc`.
const SIGNED_PREFIX: &str = "machine/";
/// License statuses that still unlock Pro.
const ACTIVE_STATUSES: &[&str] = &["ACTIVE", "INACTIVE", "EXPIRING"];
const PUBLIC_KEY_BYTES: usize = 32;

#[derive(Debug, Clone)]
pub struct KeygenMachineFiles {
    key: VerifyingKey,
}

impl KeygenMachineFiles {
    /// `public_key_hex`: the account's Ed25519 verify key, as Keygen shows it.
    ///
    /// # Errors
    /// The key isn't 32 bytes of hex or isn't a valid Ed25519 point.
    pub fn new(public_key_hex: &str) -> Result<Self, String> {
        let bytes = hex(public_key_hex.trim()).ok_or("the public key isn't hex")?;
        let bytes: [u8; PUBLIC_KEY_BYTES] = bytes.try_into().map_err(|_| "the public key isn't 32 bytes")?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|e| e.to_string())?;
        Ok(Self { key })
    }
}

fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

fn bad(reason: &str) -> LicenseError {
    LicenseError::BadFile(reason.to_owned())
}

fn time(value: &Value) -> Option<Timestamp> {
    value.as_str().and_then(parse_rfc3339)
}

impl MachineFileReader for KeygenMachineFiles {
    fn read(&self, file: &MachineFile) -> Result<Activation, LicenseError> {
        let body: String = file
            .0
            .trim()
            .strip_prefix(HEADER)
            .and_then(|rest| rest.trim_end().strip_suffix(FOOTER))
            .ok_or_else(|| bad("not a machine file"))?
            .split_whitespace()
            .collect();
        let decoded = STANDARD.decode(body).map_err(|_| bad("not base64"))?;
        let envelope: Value = serde_json::from_slice(&decoded).map_err(|_| bad("not JSON"))?;
        if envelope["alg"] != ALGORITHM {
            return Err(bad("unexpected algorithm"));
        }
        let enc = envelope["enc"].as_str().ok_or_else(|| bad("no content"))?;
        let signature =
            envelope["sig"].as_str().and_then(|s| STANDARD.decode(s).ok()).ok_or_else(|| bad("no signature"))?;
        let signature = Signature::from_slice(&signature).map_err(|_| bad("malformed signature"))?;
        // Nothing in `enc` is looked at before the signature checks out.
        self.key
            .verify(format!("{SIGNED_PREFIX}{enc}").as_bytes(), &signature)
            .map_err(|_| bad("the signature doesn't match"))?;
        let payload: Value = serde_json::from_slice(&STANDARD.decode(enc).map_err(|_| bad("content not base64"))?)
            .map_err(|_| bad("content not JSON"))?;
        let machine = &payload["data"];
        let license = payload["included"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["type"] == "licenses"))
            .ok_or_else(|| bad("no license in the file"))?;
        let attributes = &license["attributes"];
        let licensee = attributes["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .or_else(|| attributes["metadata"]["email"].as_str())
            .unwrap_or("FinderMaxxing Pro")
            .to_owned();
        Ok(Activation {
            license_id: license["id"].as_str().unwrap_or_default().to_owned(),
            licensee,
            fingerprint: machine["attributes"]["fingerprint"].as_str().ok_or_else(|| bad("no fingerprint"))?.to_owned(),
            license_expiry: time(&attributes["expiry"]),
            license_active: attributes["status"].as_str().is_some_and(|s| ACTIVE_STATUSES.contains(&s)),
            issued: time(&payload["meta"]["issued"]).ok_or_else(|| bad("no issue date"))?,
            file_expiry: time(&payload["meta"]["expiry"]),
        })
    }
}
