//! Accounts on file servers: SFTP, WebDAV and SMB. One account is one mount
//! (`sftp://4/…`): its root is the folder or share the account points at.

use std::fmt;

use crate::error::PathError;
use crate::path::{MountId, Scheme};
use crate::remote::AccountId;

pub const SFTP_SCHEME: &str = "sftp";
pub const WEBDAV_SCHEME: &str = "dav";
pub const SMB_SCHEME: &str = "smb";

/// The file-server protocols, each served by its own provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerProtocol {
    Sftp,
    WebDav,
    Smb,
}

impl ServerProtocol {
    pub const ALL: [Self; 3] = [Self::Sftp, Self::WebDav, Self::Smb];

    #[must_use]
    pub fn scheme_name(self) -> &'static str {
        match self {
            Self::Sftp => SFTP_SCHEME,
            Self::WebDav => WEBDAV_SCHEME,
            Self::Smb => SMB_SCHEME,
        }
    }

    /// # Errors
    /// Never in practice; the scheme literals are valid.
    pub fn scheme(self) -> Result<Scheme, PathError> {
        Scheme::new(self.scheme_name())
    }

    #[must_use]
    pub fn of_scheme(scheme: &Scheme) -> Option<Self> {
        Self::ALL.into_iter().find(|protocol| protocol.scheme_name() == scheme.as_str())
    }

    /// The port used when the account doesn't name one.
    #[must_use]
    pub fn default_port(self, transport: Transport) -> u16 {
        match (self, transport) {
            (Self::Sftp, _) => 22,
            (Self::WebDav, Transport::Tls) => 443,
            (Self::WebDav, Transport::Plain) => 80,
            (Self::Smb, _) => 445,
        }
    }
}

impl fmt::Display for ServerProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sftp => "SFTP",
            Self::WebDav => "WebDAV",
            Self::Smb => "SMB",
        })
    }
}

/// Only WebDAV can run without TLS; SFTP and SMB 3 bring their own encryption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    #[default]
    Tls,
    Plain,
}

/// How the app signs in. Passwords and key passphrases live in the OS keychain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerAuth {
    Password,
    /// Keys offered by the running SSH agent (SFTP only).
    Agent,
    /// A private key file, optionally protected by a passphrase (SFTP only).
    KeyFile {
        path: String,
    },
    /// Guest access (SMB) or a server without authentication (WebDAV).
    Anonymous,
}

impl ServerAuth {
    /// Whether a secret (password or passphrase) is kept in the keychain.
    #[must_use]
    pub fn uses_secret(&self) -> bool {
        matches!(self, Self::Password | Self::KeyFile { .. })
    }
}

/// Settings of a server account. Contains no secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAccount {
    pub id: AccountId,
    pub protocol: ServerProtocol,
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub transport: Transport,
    pub username: String,
    /// Where the mount's root is on the server: a start folder (SFTP), a base
    /// path (WebDAV, e.g. `remote.php/dav/files/me`) or `share/folder` (SMB).
    pub root: String,
    pub auth: ServerAuth,
}

impl ServerAccount {
    #[must_use]
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or_else(|| self.protocol.default_port(self.transport))
    }

    /// # Errors
    /// Never in practice; the scheme literal and numeric authority are valid.
    pub fn mount(&self) -> Result<MountId, PathError> {
        ServerLocation { protocol: self.protocol, account: self.id }.mount()
    }
}

/// Which account a server mount refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServerLocation {
    pub protocol: ServerProtocol,
    pub account: AccountId,
}

impl ServerLocation {
    /// # Errors
    /// Never in practice; the scheme literal and numeric authority are valid.
    pub fn mount(&self) -> Result<MountId, PathError> {
        MountId::new(self.protocol.scheme()?, &self.account.to_string())
    }

    /// `None` for mounts that are not server accounts.
    #[must_use]
    pub fn of(mount: &MountId) -> Option<Self> {
        let protocol = ServerProtocol::of_scheme(mount.scheme())?;
        Some(Self { protocol, account: AccountId(mount.authority().parse().ok()?) })
    }
}

/// An SSH server's host key, as shown to the user before trusting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    pub algorithm: String,
    /// `SHA256:…`, the form `ssh-keygen -lf` prints.
    pub fingerprint: String,
}

/// What the user decided about a server's identity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HostTrust {
    /// Only keys already known (ours or `~/.ssh/known_hosts`) are accepted.
    #[default]
    KnownOnly,
    /// Accept and remember exactly this key.
    Accept(HostKey),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_round_trip_through_mounts() {
        for protocol in ServerProtocol::ALL {
            let location = ServerLocation { protocol, account: AccountId(12) };
            let mount = location.mount().unwrap();
            assert_eq!(mount.authority(), "12");
            assert_eq!(ServerLocation::of(&mount), Some(location));
        }
        assert_eq!(ServerLocation::of(&MountId::new(Scheme::new("s3").unwrap(), "1@b").unwrap()), None);
    }

    #[test]
    fn default_ports_follow_the_protocol() {
        assert_eq!(ServerProtocol::Sftp.default_port(Transport::Tls), 22);
        assert_eq!(ServerProtocol::WebDav.default_port(Transport::Plain), 80);
        assert_eq!(ServerProtocol::WebDav.default_port(Transport::Tls), 443);
        assert_eq!(ServerProtocol::Smb.default_port(Transport::Tls), 445);
    }
}
