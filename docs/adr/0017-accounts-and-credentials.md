# ADR 0017 — Accounts in SQLite, secrets in the OS keychain, buckets by name

**Status:** accepted (M4)

## Decision
- **`AccountStore`** (SQLite, migrations 2, 4, 5) holds the non-secret parts: name, endpoint, region, auth mode
  (`DefaultChain`, `Profile(name)`, `AccessKey{id}`), addressing, `skip_cost_warnings` and extra buckets.
  Storage strings (`"access_key"`, `"path"`) are decoupled from Rust names, as in ADR 0012.
- **Ids use `AUTOINCREMENT`** and are never reused: they name keychain items, and a reused id could pick up a
  removed account's secret.
- **`CredentialStore` port**, implemented by `adapter-keyring` (keyring 4: macOS Keychain, Secret Service on
  Linux). Only the secret access key is stored there, under `CredentialKey::s3_secret(id)`. `Secret`'s `Debug`
  is redacted. A missing entry is `None`; deleting a missing entry succeeds.
- **`AddS3Account`** saves, stores the secret, connects and lists buckets; any failure rolls all of it back,
  so a wrong key never leaves a half-saved account.
- **Buckets by name:** keys scoped to one bucket can't call `ListBuckets`. *Add Bucket…* stores a name in
  `extra_buckets`; `ListBuckets` merges them and tolerates `PermissionDenied` when extras exist.
- Credentials from the default chain or a profile are resolved by `aws-config` and never copied into our store.

## Consequences
- The keychain may prompt for unsigned or rebuilt binaries. The sidebar never waits on it (ADR 0018).
- `adapter-keyring` pulls in zbus on Linux; three unmaintained-crate advisories for crates now folded into zbus
  are ignored in `deny.toml` with reasons.
