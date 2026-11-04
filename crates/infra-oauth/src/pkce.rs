//! PKCE (RFC 7636, S256) and random `state` values.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::VfsError;
use sha2::{Digest, Sha256};

/// 64 random bytes: an 86-character verifier (RFC 7636 allows 43–128).
const VERIFIER_BYTES: usize = 64;
const STATE_BYTES: usize = 24;

fn random(length: usize) -> Result<String, VfsError> {
    let mut bytes = vec![0; length];
    getrandom::fill(&mut bytes).map_err(|e| VfsError::Io { message: format!("no randomness: {e}") })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) fn verifier_and_challenge() -> Result<(String, String), VfsError> {
    let verifier = random(VERIFIER_BYTES)?;
    Ok((verifier.clone(), challenge_of(&verifier)))
}

pub(crate) fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) fn random_token() -> Result<String, VfsError> {
    random(STATE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_matches_the_rfc_example() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifiers_are_long_and_unique() {
        let (a, _) = verifier_and_challenge().unwrap();
        let (b, _) = verifier_and_challenge().unwrap();
        assert_eq!(a.len(), 86);
        assert_ne!(a, b);
    }
}
