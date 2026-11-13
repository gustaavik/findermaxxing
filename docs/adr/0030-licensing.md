# ADR 0030 — Editions and licensing (Keygen)

**Status:** accepted (M8)

## Context
FinderMaxxing is sold as a free core plus a paid Pro edition. A license must:
- work offline for weeks (a file manager is used on planes and behind captive portals);
- be limited to a number of computers, and movable between them by the user;
- survive the app being copied to another machine, the clock being set back, and the settings file being edited.

Nothing about the user may be sent except what activation needs.

## Decision
- **Vendor: Keygen.** Its license keys, machine activation and signed, offline *machine files* match the needs
  above. The account id and Ed25519 verify key are compiled in (`FM_KEYGEN_ACCOUNT`, `FM_KEYGEN_PUBLIC_KEY`,
  optionally `FM_KEYGEN_URL` for a self-hosted Keygen CE). The store link comes from `FM_STORE_URL`.
- **Editions (domain):**
  - `ProFeature` names what needs Pro: remote storage (S3, servers, cloud drives), AI search, Organize, and the
    utilities (archives, checksums, duplicates, disk usage, image tools, permissions, secure delete, the terminal).
  - Everything else is free: local browsing, file operations, previews, search, bulk rename.
- **Rules (domain, pure):** `license_status(activation, fingerprint, now)` returns one of these, checked in this
  order:
  1. `OtherMachine`: the file belongs to another fingerprint.
  2. `ClockSetBack`: `now` is before the file was issued.
  3. `Ended`: the license expired, or was suspended or banned when the file was issued.
  4. `NeedsConfirmation`: the file is past its expiry (30 days by default). The user must reconnect.
  5. Otherwise `Pro`.

  `should_renew` is true when fewer than 10 days of the file are left.
- **Ports:**
  - `LicenseServer`: activate, renew, deactivate.
  - `MachineFileReader`: verify the file, then parse it.
  - `MachineIdentity`: fingerprint and computer name.
  - `LicenseStore`: the machine file.

  The key itself lives in the OS keychain (`CredentialKey::license_key`), never in SQLite.
- **Use cases:**
  - `ActivateLicense`:
    1. Validate the key, scoped to the fingerprint.
    2. Register this machine if it isn't yet (`FINGERPRINT_TAKEN` counts as success on retries).
    3. Check out the machine file.
    4. Save the key only after the file verifies.
  - `RenewLicense`: checks the file out again when due. A license that ended on the server clears the
    activation. Network errors are silent, and the offline status stands.
  - `DeactivateLicense`: frees the seat on the server first. The local activation is removed only if that
    succeeds, so a seat is never stranded.
- **Machine file verification:** an Ed25519 signature over `machine/` + `enc` (`base64+ed25519`), checked
  before anything inside `enc` is decoded. Statuses ACTIVE, INACTIVE and EXPIRING unlock Pro.
- **Fingerprint:** SHA-256 of a salt plus the hardware UUID (`gethostuuid` on macOS, `/etc/machine-id` on
  Linux). The raw id never leaves the machine. The computer name is sent only as a label for the user's own
  machine list.
- **Gate:** `LicenseController.require(feature)` answers in QML before each Pro entry point (sidebar, Tools menu,
  AI search, terminal), and offers an upgrade (`ProDialog`) when locked.
- **Builds without an account:** debug builds unlock Pro ("Pro (developer build)"), so development and tests
  need no license. Release builds stay Free.
- **Tests:**
  - `mock-cloud` implements the client half of Keygen's API and signs real machine files (fixed seed). The
    adapter's tests cover activation, machine limits, unknown and suspended keys, deactivation, and tampered
    and forged files.
  - The use cases run against fakes.

## Consequences
- The gate is in the UI, not in the use cases. Someone patching the binary can unlock Pro, as with any
  client-side check. Moving the gate into the use cases would not change that, and would complicate every
  port's tests.
- There is no built-in trial. Keygen can issue trial licenses (a policy with a duration) through the store
  without app changes.
- Offline use is limited by the machine-file TTL, which is configured in the Keygen policy.
