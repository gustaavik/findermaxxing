# ADR 0012 — Live search now, SQLite metadata store from day one

**Status:** accepted (M3)

## Decision
- **`domain::SearchQuery`**:
  - words, all required, case-insensitive substring match on the name;
  - kind (`FileCategory`), "modified within" (past day/week/month/year) and size class;
  - hidden items opt-in.
  `SearchMatcher` resolves relative dates against an injected "now", so the rule stays pure and testable.
- **`SearchFiles` use case**: a breadth-first walk over `Lister`, so nearby results come first.
  - Hidden folders aren't descended unless asked for, and symlinked folders are never followed (no cycles, no duplicates).
  - Unreadable subfolders are counted and reported; only a missing scope is a failure.
  - Results stream as `SearchEvent`s.
  - The M6 indexer will answer indexed scopes behind the same use case; the UI won't change.
- **Search reuses the folder session.** `SessionMode::Search` streams matches into the same sorted view model, publishing progressively at most every 200 ms. Rows gain a "Folder" column relative to the scope, and sorting works the same.
- **`SavedSearchStore` port**, implemented by **`adapter-sqlite-store`** (rusqlite, bundled SQLite):
  - schema versions via `PRAGMA user_version`, each migration run once inside a transaction;
  - WAL mode and a single mutex-guarded connection on the blocking pool;
  - stored in `~/Library/Application Support/FinderMaxxing/metadata.sqlite3` or `$XDG_DATA_HOME/FinderMaxxing/`.
  The query is stored as JSON with **storage-specific names** (`"pdf"`, `"week"`), decoupled from Rust identifiers so refactors never break existing databases. Unknown values are reported as `Corrupt`, never silently dropped.
- If the database can't be opened, the app falls back to an in-memory store for the session rather than refusing to start.
