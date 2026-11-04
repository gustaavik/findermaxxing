# ADR 0004 — C++ `QAbstractListModel` shim fed by a Rust session actor

**Status:** accepted (M1) · **cxx-qt limitation flag**

## Context
The folder view must stay smooth at 100k entries. cxx-qt *can* subclass `QAbstractListModel`, but role
handling, batched `beginInsertRows` and per-row `data()` cost are easier to control in C++.

## Decision
- `cpp/entry_list_model.{h,cpp}` stores rows in a `rust::Vec<EntryRow>`. `EntryRow` is a cxx shared struct,
  pre-formatted in Rust, so `data()` is a field read plus one `QString::fromUtf8`.
- Rust computes every edit. C++ wraps it in the right model notifications: `replaceAll` (reset), `insertRowAt`,
  `removeRowAt` and `replaceRowAt`. Vector edits call back into Rust (`rows_insert` etc.) because `rust::Vec` has
  no insert or erase in C++.
- Each open folder is a `FolderSession`, a single tokio task that owns the sorted `DirectoryViewModel`, the
  listing stream and the watcher stream. It emits `ModelUpdate`s, and the controller applies them on the Qt
  thread through `CxxQtThread::queue`. A generation counter drops events from superseded sessions, and dropping a
  session cancels it.
- While loading, the sorted snapshot is published at most every 200 ms. Appending and then running a stable sort
  merges the already-sorted prefix cheaply. Watch bursts larger than 256 changes become one reset.
- The controller exposes the model as a `*mut EntryListModel` property (cxx-qt supports pointer properties).

## Workarounds found while building (cxx-qt 0.10 / Qt 6.8.3)
- All bridge files of one QML module must live in one directory (QTBUG-93443), so the bridges are flat in `src/`.
- qmltyperegistrar includes hand-written headers through two paths, so they use classic include guards, not `#pragma once`.
- The macOS 26 SDK lacks AGL's link stub, which Qt 6.8.3 still requests. `build.rs` writes a symbol-less `.tbd`
  only when the SDK lacks it, and the OS framework is used at runtime.

## Consequences
- The model has about 120 lines of C++. Everything with logic (`view_model`, `session`, `history`) is Rust and unit-tested.
- Each row stores its location string, so activation never races in-flight updates. That costs roughly 60 B per row.
