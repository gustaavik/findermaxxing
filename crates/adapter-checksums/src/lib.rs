//! `ContentHasher` on RustCrypto's MD5, SHA-1 and SHA-256 and the official
//! BLAKE3 crate.

use application::ports::{ContentHasher, RunningHash};
use domain::{Digest, HashAlgorithm};
use sha2::Digest as _;

#[derive(Debug, Default, Clone, Copy)]
pub struct StandardHasher;

impl ContentHasher for StandardHasher {
    fn start(&self, algorithm: HashAlgorithm) -> Box<dyn RunningHash> {
        match algorithm {
            HashAlgorithm::Md5 => Box::new(Crypto::<md5::Md5>(md5::Md5::new(), algorithm)),
            HashAlgorithm::Sha1 => Box::new(Crypto::<sha1::Sha1>(sha1::Sha1::new(), algorithm)),
            HashAlgorithm::Sha256 => Box::new(Crypto::<sha2::Sha256>(sha2::Sha256::new(), algorithm)),
            HashAlgorithm::Blake3 => Box::new(Blake3(Box::new(blake3::Hasher::new()))),
        }
    }
}

/// Any RustCrypto hash.
struct Crypto<H>(H, HashAlgorithm);

impl<H: sha2::Digest + Send> RunningHash for Crypto<H> {
    fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(self: Box<Self>) -> Digest {
        Digest::from_bytes(self.1, &self.0.finalize())
    }
}

struct Blake3(Box<blake3::Hasher>);

impl RunningHash for Blake3 {
    fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(self: Box<Self>) -> Digest {
        Digest::from_bytes(HashAlgorithm::Blake3, self.0.finalize().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(algorithm: HashAlgorithm, chunks: &[&[u8]]) -> String {
        let mut hash = StandardHasher.start(algorithm);
        for chunk in chunks {
            hash.update(chunk);
        }
        hash.finish().hex().to_owned()
    }

    #[test]
    fn known_answers_for_abc_in_pieces() {
        let abc: &[&[u8]] = &[b"a", b"bc"];
        assert_eq!(digest(HashAlgorithm::Md5, abc), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(digest(HashAlgorithm::Sha1, abc), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            digest(HashAlgorithm::Sha256, abc),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            digest(HashAlgorithm::Blake3, abc),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }
}
