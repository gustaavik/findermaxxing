# ADR 0005 — Qt under LGPL v3, linked dynamically; permissive-only crates

**Status:** accepted (M1) · *legal review recommended before first sale*

## Decision
- Qt is linked dynamically (frameworks on macOS, shared libraries on Linux). The bundle ships unmodified Qt
  libraries plus the LGPL text and a notice explaining how users can replace them.
- Only LGPL Qt modules are used: Core, Gui, Qml, Quick and Quick Controls 2 (pulled in by `cxx-qt-lib`), plus the
  image-formats plugin. GPL-only modules (for example Qt Charts or Qt Virtual Keyboard in some versions) are off-limits.
- Crates must be permissively licensed (`deny.toml`), and `cargo xtask licenses` generates the third-party notice.
- Development builds embed an rpath to the local Qt (`crates/app/build.rs`). Release bundles get theirs from macdeployqt/linuxdeploy.

## Open points for you
- **Qt 6.8 LTS patch releases after 6.8.3 are commercial-only.** Staying on open-source 6.8.3 means no further
  security fixes. The alternative is following the current open-source 6.x line (6.10/6.11) and re-validating per
  release. Decide before M8.
- On macOS, signing and notarizing a bundle with LGPL frameworks is fine. Don't statically link or modify Qt without a commercial licence.
