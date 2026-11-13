//! Licensing on Keygen (ADR 0030).
//!
//! - [`KeygenServer`] (`LicenseServer`): validates the key scoped to this
//!   machine, activates the machine if needed, and checks out a signed
//!   machine file (`base64+ed25519`), authenticating with the license key
//!   itself (`Authorization: License <key>`), so the app holds no API token.
//! - [`KeygenMachineFiles`] (`MachineFileReader`): verifies the file's
//!   Ed25519 signature over `machine/<enc>` with the account's public key,
//!   then reads the machine, its license and the file's validity window.
//! - [`HostIdentity`] (`MachineIdentity`): a SHA-256 of the hardware UUID
//!   (macOS) or `/etc/machine-id` (Linux), never the id itself.

mod files;
mod identity;
mod server;

pub use files::KeygenMachineFiles;
pub use identity::HostIdentity;
pub use server::{KeygenAccount, KeygenServer};
