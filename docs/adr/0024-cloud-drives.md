# ADR 0024 — Google Drive, Dropbox and OneDrive as providers

**Status:** accepted (M5b)

## Decision
- **Accounts:** `domain::DriveAccount` (provider, name, email), SQLite `drive_accounts` (AUTOINCREMENT, as ADR
  0017); one mount per account (`gdrive://<id>/`, `dropbox://<id>/`, `onedrive://<id>/`). `DriveConnector` per
  provider: `sign_in` (browser, ADR 0023) → `AddDrive` saves only if `connect` then works, rolling back
  otherwise. Saved drives connect concurrently at start-up.
- **Providers** (all pass the lister, mutation, stream and resume contracts; Drive also the Trash contract):

  | Provider | Addressing | Writes and resume | Notable |
  |---|---|---|---|
  | `adapter-gdrive` (Drive API v3) | ids; paths resolved folder by folder through briefly cached listings | resumable upload sessions in 8 MiB chunks; the file appears on the last chunk; the state is the session URL, resumed with a `bytes */*` status query | server-side copy; Drive's own Trash with exact restore; Google Docs exported (see below); duplicate names disambiguated (see below) |
  | `adapter-dropbox` (API v2) | case-insensitive paths | `upload`, or upload sessions committed with `mode: add`; resume asks with offset 0 and uses Dropbox's `correct_offset`; discard *closes* the session (sessions can't be deleted; they expire after a week, unbilled) | server-side copy and move; header arguments `\u`-escaped |
  | `adapter-onedrive` (Microsoft Graph) | `root:/path:` for lookups, ids for changes | small files in one request; larger ones in pre-authorised upload sessions (no bearer token, 10 MiB = 32 × 320 KiB chunks, the total in every chunk, so the state is `"<total> <url>"`); `conflictBehavior: fail` | asynchronous server-side copy behind a monitor URL; downloads redirect to pre-authorised URLs; deletes go to the OneDrive recycle bin |
- **No overwrite:** every write first checks that the name is free, asking the provider rather than a cache.
  Commits use `mode: add` / `conflictBehavior: fail` where the API has them. Drive has no such option, so a
  write racing another client can create a duplicate, which then shows as `name (2)`.
- **Duplicate names (Drive):** `domain::disambiguate` shows later duplicates as `report (2).pdf`, oldest first
  (`orderBy=createdTime`), skipping names another item uses. Nothing is renamed on Drive, and every item stays
  addressable by its shown name.
- **Google Docs/Sheets/Slides/Drawings** show as `Name.docx/.xlsx/.pptx/.png`, without a size. Reading exports
  them. Renaming or moving keeps the Drive name without the suffix. Forms, sites and shortcuts are listed as
  "other" and can't be read.
- **Caching, offline pins and the resume journal** apply as for every remote provider (ADRs 0019, 0020).
- **Testing before real accounts exist:**
  - `mock-cloud` is one local HTTP server with an OAuth authorization server and the subsets of the three APIs
    the adapters use.
  - It follows their documented behaviour, including the awkward parts: duplicates, 256 KiB/320 KiB chunk rules,
    `incorrect_offset`, closed sessions, upload URLs that reject the bearer token, 302 downloads, asynchronous
    copies.
  - Each contract fixture starts its own mock, and fixture setup writes straight into the mock's tree.
  - `live_round_trip` (ignored) runs against the real services once client IDs exist.

## Consequences
- Until the live tests have run, behaviour against the real services is only as good as the doubles' reading
  of the documentation. That's the main M5b risk, and the first thing to check with real IDs.
- Drive's listing cache (3 s) can show another client's change late; writes never rely on it.
- iCloud Drive stays a local folder (M5a); Apple has no third-party API for it.
