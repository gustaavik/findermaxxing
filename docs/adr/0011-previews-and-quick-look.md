# ADR 0011 — Previews: a Qt pane everywhere, native Quick Look on macOS

**Status:** accepted (M3)

## Decision
- **`PreparePreview` use case** classifies an item (`domain::FileCategory`, by extension) and gathers what the pane needs: a local file for image/PDF/audio/video, highlighted HTML for text and code, a listing for archives, an item count for folders, metadata otherwise. Unknown files under 512 KB are sniffed as text. Rendering failures degrade to a metadata summary and never surface as errors.
- **`Materializer` role** (`Capability::LocalAccess`): "give me a local file for this item". `LocalFs` returns the file itself. In M4 remote providers implement it as a cached download, so previews and Quick Look work unchanged.
- **Ports for the heavy parts, implemented in `adapter-preview`:**
  - `TextRenderer` uses syntect: the first 256 KB and 2,000 lines only, NUL means binary, inline-styled HTML with light/dark themes. Grammars load lazily on first use, so start-up is unaffected.
  - `ArchiveInspector` lists zip (central directory only, nothing decompressed), tar and tar.gz. 7z, rar, xz and zstd come with the archive utilities in M7.
- **Pane** (`PreviewPane.qml`): Qt `Image` decoded at display size, `PdfMultiPageView` (QtPdf), `MediaPlayer` + `VideoOutput` (QtMultimedia), and a read-only rich-text view. Selection changes are debounced (120 ms) with a request counter, so arrow-key scrolling never renders stale items.
- **Quick Look** (Space on macOS): an Objective-C++ shim (`cpp/quick_look_mac.mm`) — **a cxx-qt limitation workaround**. `QLPreviewPanel` needs a controller in the window's responder chain, and Qt's `QNSView` isn't one. The shim adds `acceptsPreviewPanelControl:`, `beginPreviewPanelControl:` and `endPreviewPanelControl:` to the view's class at runtime (idempotent `class_addMethod`) and serves the selection's file URLs as the data source. It's built without ARC, so no Objective-C flags leak into the C++ build. On Linux, Space toggles the pane instead.

## Consequences / limits
- Quick Look shows the selection as of opening; the panel's own arrow keys don't move the app's selection yet.
- Office/iWork documents preview only through Quick Look on macOS; on Linux they show metadata.
