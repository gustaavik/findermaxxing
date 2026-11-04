# ADR 0006 — Batched, back-pressured streaming listings

**Status:** accepted (M1)

## Decision
- `Lister::list` yields `EntryBatch`es of at most `LIST_BATCH_SIZE = 512`.
- `LocalFs` reads directories on the runtime's blocking pool and sends batches over a bounded channel
  (4 in flight). A slow UI applies back-pressure instead of the whole listing being buffered twice.
- Cancellation is checked between entries, and dropping the consumer stops the reader.
- The watcher subscribes *before* the listing starts, so changes during a long listing aren't lost.
- FSEvents reports canonical paths (`/private/var/...`), so the local watcher matches events against the
  canonicalised directory and re-`lstat`s each path instead of trusting coalesced flags.

## Measured (M1, macOS, release build, APFS)
100,000 files: **first batch in 2.3 ms, full listing in 484 ms** (`cargo test --release -p adapter-local-fs --test local_fs -- --ignored large_dir`).

## Known limitations
- Names that aren't valid UTF-8 (possible on Linux) are skipped, because `PathSegment` is a `String`.
  A byte-preserving name type is a decision for M2.
- Every `FileEntry` stores its full `StoragePath` (cloned parent segments). Memory is acceptable at 100k but
  could shrink with an `Arc`-shared parent.
