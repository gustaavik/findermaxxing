# ADR 0008 — Mutating roles, never-overwrite contract, platform trash

**Status:** accepted (M2)

## Decision
- New roles: `Copier` (one non-directory item), `Mover` (rename), `Remover` (permanent), `FolderMaker`, `Trasher` (trash + exact restore) and `Launcher` (open with default app). `MountHandle` holds each as an optional role. A macro generates the slot, the builder method and the `Unsupported(capability)` accessor, so adding a role is one line.
- **Contract: nothing overwrites.** Copy uses `O_EXCL` (`create_new`). Rename uses `renamex_np(RENAME_EXCL)` on macOS and `renameat2(RENAME_NOREPLACE)` on Linux, falling back to check-then-rename where a filesystem lacks it. A case-only rename (`a.JPG` → `a.jpg` on case-insensitive APFS) is detected by inode and allowed.
- **Copy:** APFS `clonefile` first (instant, keeps metadata); otherwise a 1 MiB chunked copy with progress and cancellation per chunk, then permissions and mtime are copied over. A partial destination is always removed. Symlinks are copied as links.
- **Cross-device moves** surface as `VfsError::CrossDevice`. The application then copies and deletes, so providers never decide that policy.
- **Trash** lives inside `adapter-local-fs` as `LocalTrash`. This deviates from the plan's separate crates: trash is a local-filesystem role and needs the same `file://` ↔ native mapping, and splitting it out would force adapter-to-adapter dependencies.
  - macOS uses `NSFileManager trashItemAtURL:resultingItemURL:` (objc2), the same call Finder uses, which handles per-volume `.Trashes` and returns the exact destination.
  - Linux follows freedesktop Trash 1.0: home trash on the same device, otherwise `$topdir/.Trash/$uid` (sticky, admin-made) or `$topdir/.Trash-$uid`. It writes `.trashinfo` with `O_EXCL` slot reservation, a percent-encoded `Path=` and a local-time `DeletionDate`. It never copies across devices.
  - Restore on both is a no-replace rename back. `NotInTrash` means the item was emptied; `AlreadyExists` means the original place is taken again.
- **Launcher:** `open` (macOS) or `xdg-open` (Linux). The child is reaped on its own thread, because some xdg handlers block until the app exits.

## Proven by
`vfs-contract-tests`: `mutation_contract_tests!` (13 cases) and `trash_contract_tests!` (4 cases, run against the **real** platform trash and fully restored). Both run for `LocalFs` and for `FakeVfs`. Unit tests cover the chunked copy, mid-copy cancellation, symlink copy and the freedesktop layout (runs on macOS against a temp trash).
