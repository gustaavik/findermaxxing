# ADR 0025 — Content index and semantic search

**Status:** accepted (M6)

## Context
People remember what a file says or shows more often than its name: "the electricity invoice from March",
"photos of the cats", in Danish as often as in English. Search by name (ADR 0012) can't answer that. The answer
has to stay on the machine, run in the background without being noticed, and cover any provider.

## Decision
- **Ports** (`application::ports::semantic`): `TextEmbedder`, `ImageEmbedder`, `ContentExtractor`,
  `SemanticIndex`, `AiSettingsStore`, `ModelInstaller`, `IndexObserver`. The core knows vectors and
  similarities, never a model runtime (ADR 0026 has the adapter). Each index entry records the `ModelId` that made
  it. Spaces of different models never mix, and a model change re-indexes.
- **What is indexed** (`domain::indexing`):
  - **Scope:** an `IndexScope` of include and exclude folders; excludes win. The default is
    Documents/Desktop/Downloads/Pictures. Folders on other providers can be added; they are read through
    `Materializer`, so the file cache applies (ADR 0020).
  - **Skipped:**
    - hidden items;
    - dependency and build folders (`node_modules`, `.git`, `target`, …);
    - bundles (`.app`, `.photoslibrary`, …);
    - lock files and minified bundles (`package-lock.json`, `*.lock`, `*.min.js`, …);
    - text over 20 MB and images over 40 MB.
  - **Text:** cut into overlapping passages (1000 characters, 150 overlap, at most 48 per document).
  - **Images:** indexed only after the separate image opt-in.
  - **Change detection:** a `Fingerprint` of size and modified time. Items with nothing to index are remembered
    too, so they aren't read again until they change.
- **The indexer** (`use_cases::indexer`): one long-lived task, steered through a cloneable `IndexerControl`.
  - **A pass:**
    1. Load the settings; stop there if AI search is off, paused, or its model is missing.
    2. Walk each include root breadth-first. Forget items that are gone, and items now outside the scope.
       Unreadable folders keep what was already indexed below them.
    3. Index new and changed items one at a time, with a 40 ms pause after each.
  - **Commands** ("settings changed", "rescan") are checked between files. A command interrupts the pass, and the
    next pass starts from fresh settings.
  - **Rescans** run every 15 min, after file operations, and when the user asks. There is no file watching yet.
- **Consistency under concurrency.** `IndexerControl` carries two async locks:
  - **Settings changes** take the first one for their whole load, edit and save, so concurrent changes can't
    overwrite each other.
  - **Index writes** take the second one. The indexer holds it while it re-checks its commands and writes an
    item. Clearing the index sends the "settings changed" command first, then takes the lock. A write that was
    already in flight therefore lands before the clear, never after it, and an item embedded across a settings
    change is dropped.
  - Both races were found in the M6 smoke test; `application/tests/semantic.rs` reproduces them with ports that
    yield halfway.
- **Queries** (`domain::natural_query`): `parse_query` takes a `QueryClock` (now plus the UTC offset) and splits
  out the following. What's left is embedded.
  - **Dates**, in English and Danish: "last week", "i marts", "summer 2025", "i går", …
  - **Kind words**: "photos", "billeder", "PDFs", "documents", …
    - "Documents" means anything people read: notes, PDFs and office files (`category_matches`).
    - "PDFs" means only PDFs.
  - **Date or kind only** ("documents from this week"): the filters alone apply, newest first.
- **Ranking** (`use_cases::semantic_search`):
  - Text and image results are ranked separately, each above its model's measured relevance floor.
  - The two lists are fused by reciprocal rank (k = 20).
  - A document's score is its best passage, nudged up when its name contains query words.
  - Results stream into the folder view as a search session in **Contents (AI)** mode, sorted by relevance.
- **Storage** (`adapter-sqlite-store`, migration 10): `semantic_items`, `semantic_passages` and `ai_settings`.
  - Vectors are stored as int8 with a per-vector scale, a quarter of the size of f32.
  - They are loaded into memory at start-up and searched by brute force. For a personal index (10⁴–10⁵ passages
    × 384 dims) that takes milliseconds, and it avoids an ANN index and its native dependency.
- **Off means off:** "Turn off and delete" clears the index and removes the model packs from disk.

## Consequences
- **Change detection lags:** changes made outside the app are seen at the next rescan (≤ 15 min). File watching
  across a whole scope (FSEvents/inotify limits) is a later step.
- **Scale:** brute force holds to roughly 10⁶ passages; past that we need an ANN index behind the same port.
- **Snippets** are stored but not shown in the UI yet.
