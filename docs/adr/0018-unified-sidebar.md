# ADR 0018 — One sidebar for places, favorites and remote accounts

**Status:** accepted (M4)

## Decision
- **Rows:** `Heading`, `Place` (home, standard folders, mounted volumes via `VolumeLister`), `Favorite`
  (`FavoritesStore`, any location including S3 prefixes), `Account` and `Bucket`. Built in Rust
  (`sidebar_rows`) and pushed into a C++ `SidebarModel`, like the folder model (ADR 0004).
- **Local first:** `refresh` publishes places and favorites immediately, then accounts and their buckets as each
  connects. A slow endpoint or a keychain prompt can't hold up the sidebar.
- A failed account shows its error as the row detail instead of disappearing.
- Add Account offers presets (Amazon S3, self-hosted MinIO/SeaweedFS, Cloudflare R2, Backblaze B2, Wasabi) that fill endpoint and
  addressing; Remove Account deletes the row and its keychain item.
- **Remote items and the clipboard:** the system clipboard only carries `file://` URLs other apps understand.
  Copying S3 items puts their `s3://` URLs there too, and keeps an **in-app clipboard** of locations that paste
  uses as long as the system clipboard still holds the text we put there.

## Consequences
- Drag and drop between the sidebar and views, and to other apps for S3 items, is M5 work.
