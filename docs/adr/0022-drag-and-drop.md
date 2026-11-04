# ADR 0022 — Drag and drop with the OS, for local and remote items

**Status:** accepted (M5a)

## Decision
- **Drags are OS drags** (`Drag.Automatic`), so items can land in Finder, mail, chat apps or another window.
  The mime data carries:
  - `text/uri-list`: local `file://` URLs for other apps. Remote items are downloaded into the cache first
    (`prepare_drag` → `drag_ready`), and the drag starts once they're ready if the button is still down;
  - `application/x-findermaxxing-locations`: our own lossless locations, so a drop inside the app moves or copies
    the *real* remote items, not their cached copies.
- **Drops** on a folder view (or a folder row in it) or on a sidebar folder go through `drop_items`, which
  takes our own locations when present and file URLs otherwise. Items dropped into the folder they came from
  are ignored.
- **Copy or move** follows Finder: within one mount a drop moves, across mounts it copies. ⌥ forces a copy and
  ⌘ a move on macOS; Ctrl/Shift on Linux. The modifiers are read from a C++ shim
  (`QGuiApplication::queryKeyboardModifiers`), because QML's `DragEvent` doesn't carry them: **cxx-qt/QML
  limitation**.
- Drops are ordinary copy/move jobs: conflicts, costs, progress, journaling and undo all apply.

## Consequences
- Dragging remote *folders* out to other apps isn't supported (only files are materialised); inside the app
  they work.
- Lesson: QML puts every enum of a type in one namespace. `DropOperation` uses `FollowKeys`/`AlwaysCopy`/
  `AlwaysMove` because a `Copy` value silently shadowed `PasteMode.Copy` and turned Paste into a move. The smoke
  test caught it.
