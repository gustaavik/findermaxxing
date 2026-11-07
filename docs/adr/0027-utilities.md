# ADR 0027 — Utilities: archives, checksums, duplicates, disk usage, images, permissions, secure delete

**Status:** accepted (M7a)

## Context
The spec's utility set has to fit the architecture rather than sit beside it. Each tool should:
- work on every provider where the operation makes sense;
- report progress, cancel, and undo through the existing job machinery;
- keep libraries (C or Rust) behind ports.

## Decision
- **Ports** (`application::ports::utilities`), with the adapters that implement them:

  | Port | Adapter | Library |
  |---|---|---|
  | `ContentHasher` / `RunningHash` | `adapter-checksums` | RustCrypto MD5, SHA-1, SHA-256; `blake3` |
  | `ArchiveExtractor`, `ArchiveBuilder` | `adapter-archives` | zip; tar with gzip, bzip2, xz or zstd; 7z (sevenz-rust2); RAR read-only (unRAR) |
  | `ImageConverter`, `ImageFingerprinter`, `ImageMetadata` | `adapter-imaging` | image, image_hasher, kamadak-exif, img-parts |
  | `PermissionEditor` (provider role) | `adapter-local-fs` (`chmod`), `adapter-sftp` (`setstat`) | — |
  | `SecureEraser` (provider role) | `adapter-local-fs` | — |

- **Provider-neutral by construction.** Archives and images are read through `Materializer`, which gives a local
  copy (a cached download for remote items). Every result goes back through the destination's `Writer` and
  `FolderMaker`, so:
  - compressing a bucket folder into a server folder works;
  - extracting a zip that sits on Google Drive works;
  - checksums and duplicate hashing stream through `Reader`, without downloading whole files first.
- **Archives as event streams.** Extraction emits `Folder` / `File` / `Chunk` / `EndFile` / `Skipped` events,
  and building consumes the same events.
  - **Threads and flow control:** the libraries are blocking, so each job runs on tokio's blocking pool behind
    bounded channels. Memory stays flat, and dropping the stream stops the work.
  - **Building:** archives are built in an anonymous temporary file, because zip and 7z need to seek. tar and 7z
    entries are spooled first, because they need the length up front; a file that changes while being packed
    therefore can't corrupt the archive.
  - **Metadata kept:** modification times and Unix modes go into zip, tar and 7z; execute bits come back out on
    extraction.
- **Safety:**
  - `domain::entry_path` refuses `..` anywhere and drops leading `/` and drive letters (zip-slip).
  - Links and special files are skipped and reported ("N items weren't extracted").
  - Encrypted archives give a clear message.
  - RAR is never offered as an output format (`ArchiveFormat::can_create`).
- **Undo.** Two new operations:
  - `Operation::Created(paths)`: undo trashes them. Used by compress, extract and convert.
  - `Operation::PermissionsChanged(path, before, after)`: undo restores each item's previous mode through
    `Action::SetPermissions`.

  Cleaning metadata trashes the original and puts the cleaned copy at its name, recorded as `Trashed` + `Created`;
  undo removes the copy and restores the original byte for byte. Secure delete is deliberately not undoable.
- **Checksums:**
  - all four digests are computed in one pass;
  - pasted digests are recognised by length (an optional `sha256:` prefix is ignored);
  - published lists next to the file are found and checked automatically: `SHA256SUMS`, `name.sha256`, BSD
    format;
  - a match with MD5 or SHA-1 carries a note that it proves integrity, not authenticity.
- **Duplicates.** Stages, cheapest first:
  1. same size;
  2. same BLAKE3 of the first 64 KiB;
  3. same BLAKE3 of the whole file;
  4. optionally, images within 6 bits of an 8×8 gradient hash ("look alike").

  The suggested keeper is the oldest copy, then the one with the shortest path. Removal goes through the Trash
  and can be undone.
- **Disk usage.** One walk builds a `UsageNode` tree. Each folder keeps its 40 largest files and sums the rest
  into one "N smaller files" node, so memory is bounded by the number of folders. The squarified treemap layout
  is pure Rust in `presentation::treemap`; QML only places rectangles.
- **Images:**
  - **Conversion:** to JPEG (quality), PNG, lossless WebP, TIFF, BMP or GIF; resized with Lanczos3 and never
    enlarged. EXIF orientation is applied first, so portrait photos stay upright.
  - **Metadata:** EXIF fields are classified as location (GPS) or identity (names, serial numbers) and flagged.
  - **Stripping:** done at container level (JPEG APP1/APP13/COM, PNG text/eXIf/tIME, WebP EXIF/XMP), so pixels
    and the ICC colour profile are untouched.
- **Permissions:**
  - **Edits:** each bit is set, cleared or kept, so mixed selections stay mixed where the user didn't click.
  - **Reach:** the selected items only, everything inside, or only the files or only the folders inside.
- **Secure delete:**
  - one pass of random data, `fsync`, truncate, rename to a random name, then remove;
  - local disks only, as the `SecureErase` role;
  - the confirmation says plainly that SSDs and copy-on-write file systems (APFS, Btrfs) can keep old blocks,
    and that disk encryption is the real protection.
- **RAR licensing.** The `unrar` crate vendors RARLAB's unRAR source under a freeware licence. It allows use in
  closed-source software and forbids creating RAR archives, which we never do. cargo-deny can't see this licence
  (the crate says MIT OR Apache-2.0), so its text is added to the third-party report directly (`about.hbs`).
  Libbzip2's own licence (`bzip2-1.0.6`, BSD-style) is allowed in `deny.toml`.

## Consequences
- **Extraction progress:** a bar can't show the share done (archive sizes aren't known up front), only the
  current item and bytes written. Extracted files get today's date: `Writer` can't set modification times.
- **Password-protected archives** are refused for now; asking for a password would be a later addition to the
  ports.
- **HEIC/AVIF photos** can't be converted yet; the `image` crate has no decoder for them.
- **Large disk-usage scans** keep the whole folder tree in memory (about 200 bytes per folder).
