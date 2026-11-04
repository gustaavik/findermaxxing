# ADR 0020 — One size-limited cache for every remote provider; offline pins

**Status:** accepted (M5a)

## Decision
- **Caching is a decorator, not a provider feature.** `adapter-file-cache::CachingResolver` wraps the mount
  registry. A mount that can be read but has no local files (S3, SFTP, WebDAV, SMB) gets:
  - a `CachedMaterializer` (previews, Quick Look, Open, drag-out);
  - a `CachingLister`.
  The local disk is left alone. S3's own M4 download cache is gone.
- **Freshness:** a copy is reused while the remote item's size and modification time match. Copies keep their
  real names (`<cache>/<scheme>/<authority>/<path>`), so opened files show the right name and type.
- **`CacheIndex` port** (SQLite `cache_entries`, `cache_listings`) holds the bookkeeping; the bytes live in
  `~/Library/Caches/FinderMaxxing/files` (`~/.cache/FinderMaxxing/files` on Linux).
- **Eviction** is a pure domain rule, `CachePolicy::evictions`: least recently used first, until the cache fits
  **5 GB**. Pinned copies count towards the total but are never evicted.
- **Offline pins** (`PinStore`, SQLite `pins`):
  - *Keep Available Offline* pins an item (folders recursively), downloads everything, and stores each folder's
    listing.
  - When the provider can't be reached (`Transient`/`Io`), or the mount can't even be opened, pinned folders are
    listed from those snapshots and pinned files open from the cache. This offline view is read-only.
  - A snapshot is only stored from a complete listing.
- **Removing an account** (`CacheJanitor`, `ForgetLocalCopies`) deletes its cached copies, stored listings and
  pins: nothing it downloaded stays on the computer.

## Consequences
- Adding a provider gets caching and offline access for free (Open/Closed).
- The limit is a value; a settings screen (M8) can change it. There is no per-pin "sync"; a pinned folder is
  refreshed whenever it is listed online.
- Non-UTF-8 remote names are stored lossily as cache file names (two such names could share a copy; rare, and
  only ever a cache miss).
