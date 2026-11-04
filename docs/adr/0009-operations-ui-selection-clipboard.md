# ADR 0009 — OperationsController, selection in the model, system clipboard

**Status:** accepted (M2)

## Decision
- **One `OperationsController` per window** (cxx-qt) starts jobs, owns their `CancellationSource`s, shows progress in a C++ `JobListModel`, queues conflict questions, maps errors to text and exposes undo/redo labels. Use cases decide everything about files.
- **Job ports implemented in presentation:**
  - `QtJobObserver` throttles progress to 10 Hz, always delivering 100 %.
  - `QtConflictResolver` sends the question to the Qt thread and awaits a oneshot answer. A dropped answer (window closed, job cancelled) means *Stop*.
  - Cancelling a job also answers its pending questions with Stop; otherwise a job blocked on a dialog could never end.
- **Selection is view state in `EntryListModel`** (C++): a `selected` role plus `selectOnly`, `toggle`, `extendTo`, `selectAll` and `selectedLocations`. It follows inserts and removes and survives resets by location, so a re-sort or a watcher burst doesn't drop it. QML stays free of selection bookkeeping.
- **Clipboard = the system clipboard with file URLs** (plus plain-text paths), so copy/paste interoperates with Finder, Nautilus, Dolphin and terminals. Paste copies; *Move Here* (Cmd/Ctrl+Option/Alt+V, Finder's convention) moves. Remote locations will need an in-app clipboard (M4).
- Files activated in a folder view raise `openRequested`, which the window's `OperationsController` turns into the `OpenEntry` use case.

## Known limitations
- Job, undo and conflict wording is built in Rust and English-only. It becomes translation keys when the app is localised; QML strings already use `qsTr`.
- Drag-and-drop (between panes and to and from other apps) isn't in M2. It needs a C++ shim for native drags (cxx-qt cannot start platform drags) and is planned with split panes.
