# ADR 0007 — Byte-preserving names and two string forms for locations

**Status:** accepted (M2) · resolves the M1 limitation in ADR 0006

## Context
Linux filenames are arbitrary bytes except `/` and NUL. In M1 a `String` segment meant such files were invisible. A file manager must not hide or mangle files.

## Decision
- `PathSegment` stores raw bytes and keeps the same invariants (not empty, `.` or `..`; no `/` or NUL). It offers `as_bytes()`, `as_str() -> Option<&str>` and a lossy `display_name()`.
- `StoragePath` has two string forms:
  - **human**: `parse` and `Display`. Taken literally, never decoded, so typing `/tmp/50%25` means exactly that. Invalid bytes display as U+FFFD.
  - **lossless**: `encode` and `decode`. Only `%` and invalid UTF-8 bytes are percent-escaped. Every string-only channel (Qt model rows, signals) uses this form.
- The local adapter converts through `OsStrExt::{as_bytes, from_bytes}`, which is Unix-only by design.
- Clipboard file URLs are a third, RFC 3986 form built in `presentation::file_urls`, because a raw `#` or `?` would break a URL.
- Sorting compares display names naturally, then raw bytes, so the order stays total.

## Consequences
Every file is visible and operable, including copy, move, trash and rename. Remote providers (S3, cloud) use UTF-8 keys and never hit the lossy path.
