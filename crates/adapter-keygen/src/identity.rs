//! This computer's fingerprint: a salted SHA-256 of its hardware UUID
//! (macOS) or systemd/D-Bus machine id (Linux). Stable across reinstalls of
//! the app, and the raw id never leaves the machine.

use application::ports::MachineIdentity;
use sha2::{Digest, Sha256};

/// Keeps our fingerprints unrelated to other apps' hashes of the same id.
const SALT: &str = "FinderMaxxing machine v1:";
#[cfg(not(target_os = "macos"))]
const MACHINE_ID_FILES: &[&str] = &["/etc/machine-id", "/var/lib/dbus/machine-id"];
const FALLBACK_NAME: &str = "Computer";

#[derive(Debug, Clone)]
pub struct HostIdentity {
    fingerprint: String,
    name: String,
}

impl HostIdentity {
    /// `None` when the machine has no stable id to read.
    #[must_use]
    pub fn detect() -> Option<Self> {
        let id = hardware_id()?;
        let digest = Sha256::digest(format!("{SALT}{}", id.trim()).as_bytes());
        let fingerprint = digest.iter().map(|b| format!("{b:02x}")).collect();
        Some(Self { fingerprint, name: host_name().unwrap_or_else(|| FALLBACK_NAME.to_owned()) })
    }
}

impl MachineIdentity for HostIdentity {
    fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

#[cfg(target_os = "macos")]
fn hardware_id() -> Option<String> {
    let mut uuid = [0_u8; 16];
    let wait = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `uuid` is 16 writable bytes, as `gethostuuid` requires.
    let result = unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &raw const wait) };
    (result == 0 && uuid.iter().any(|b| *b != 0)).then(|| uuid.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(not(target_os = "macos"))]
fn hardware_id() -> Option<String> {
    MACHINE_ID_FILES.iter().find_map(|path| std::fs::read_to_string(path).ok().filter(|id| !id.trim().is_empty()))
}

fn host_name() -> Option<String> {
    let mut buffer = [0_u8; 256];
    // SAFETY: the buffer is valid for its length; the call NUL-terminates within it.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return None;
    }
    let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
    let name = String::from_utf8_lossy(&buffer[..end]).trim_end_matches(".local").to_owned();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_has_a_stable_hashed_fingerprint() {
        let (first, second) = (HostIdentity::detect(), HostIdentity::detect());
        let first = first.map(|i| i.fingerprint());
        assert_eq!(first, second.map(|i| i.fingerprint()));
        assert!(first.is_some_and(|f| f.len() == 64 && f.chars().all(|c| c.is_ascii_hexdigit())));
    }
}
