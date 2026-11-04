# ADR 0021 — SFTP, WebDAV and SMB as providers; one account model for servers

**Status:** accepted (M5a)

## Decision
- **One account model:** `domain::ServerAccount` (protocol, host, port, transport, user, root, sign-in) in
  SQLite `server_accounts` (AUTOINCREMENT ids, as ADR 0017). Passwords and key passphrases go to the keychain
  under `CredentialKey::server_secret`. A mount per account: `sftp://<id>/`, `dav://<id>/`, `smb://<id>/`. The
  account's root (start folder, WebDAV base path, `share/folder`) is the mount root.
- **`ServerConnector` port**, one per protocol in `ServerServices.connectors`. Adding a protocol is adding a
  connector and a factory (Open/Closed). `AddServer` saves only if signing in works and rolls everything back
  otherwise; `ConnectServer` reconnects saved servers concurrently, so a dead host never delays the sidebar.
- **Host keys (SFTP):**
  - keys known to `~/.ssh/known_hosts` (read only) or the app's own `known_hosts` are trusted;
  - a new key fails with `ConnectError::UnknownHost(fingerprint)`; the UI shows the SHA256 fingerprint, and only
    `HostTrust::Accept(that exact key)` connects and remembers it;
  - a changed key is always refused, with a warning.
- **Providers (all pass the lister, mutation, stream and, where resumable, resume contracts against real
  servers in Docker):**

  | Provider | Crate (licence) | Notes |
  |---|---|---|
  | `adapter-sftp` | russh + russh-sftp (Apache-2.0) | password, SSH agent, or key file; writes via hidden partial + rename (SFTP v3 rename never replaces); copies stream (v3 has no server-side copy); handshakes retry, since OpenSSH's `MaxStartups` drops connection bursts |
  | `adapter-webdav` | reqwest + rustls with the OS trust store, quick-xml (MIT/Apache) | PROPFIND/GET/PUT/MOVE/COPY/MKCOL/DELETE; every write `Overwrite: F`; uploads go to a hidden partial that is MOVEd into place; server-side COPY; not resumable |
  | `adapter-smb` | smb2 (MIT/Apache, pure Rust, SMB 2/3) | auto-reconnect; exclusive create; partial + no-replace rename; resumable; copies stream for now |

  Samba's libsmbclient is GPL and can't ship in a closed-source app. Shares the OS mounted (Finder, gvfs) also
  appear under Locations.
- **RSA keys:** russh is built without its `rsa` feature. The `rsa` crate's private-key operations aren't
  constant-time (RUSTSEC-2023-0071, no fix), and signing with an RSA key file would expose that to the server.
  RSA keys still work through ssh-agent, which signs on its own. Ed25519/ECDSA keys and host keys are
  unaffected. Servers offering *only* RSA host keys (OpenSSH before 6.5) can't be reached.
- **Local cloud folders:** iCloud Drive and File Provider folders (`~/Library/CloudStorage`: Google Drive,
  OneDrive, Dropbox, Box…) appear under Locations as local folders; gvfs mounts too on Linux.

## Consequences
- No change notifications for servers yet (SMB could use CHANGE_NOTIFY); views reload after jobs, as with S3.
- SFTP and SMB can't address names that aren't valid UTF-8 (their wire formats are strings).
- A dropped connection isn't re-established in the background for SFTP and WebDAV; *Reconnect* in the
  sidebar does it. SMB reconnects by itself.
