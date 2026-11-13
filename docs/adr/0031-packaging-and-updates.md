# ADR 0031 — Packaging, signing and updates

**Status:** accepted (M8)

## Context
Users install from a DMG on macOS, and from an AppImage or Flatpak on Linux. Each platform has its own update
convention, and macOS refuses unsigned, unnotarized apps. Qt is LGPL (ADR 0005): users must be able to replace
its libraries. The source repository is private, but the binaries are public.

## Decision
- **macOS** (`cargo xtask package-macos [--universal] [--sign ID] [--notarize PROFILE]`):
  1. Release build. `--universal` builds arm64 and x86_64 and merges them with `lipo` (Qt's frameworks are
     already universal).
     - `MACOSX_DEPLOYMENT_TARGET=14.0` is set, or C++ objects would target the SDK's macOS version.
     - On arm64, `<arm_acle.h>` is force-included to work around a header bug in Qt 6.8.3 (`qyieldcpu.h`).
  2. Bundle with `Info.plist` (`dev.findermaxxing.FinderMaxxing`, minimum macOS 14, folder-access reasons) and
     the icon. Then `macdeployqt -qmldir`, after which the unused SQL driver plugins are removed.
  3. Embed Sparkle 2.10.1, downloaded once and pinned by SHA-256.
  4. Sign inside out (nested code first, symlinks skipped) with the hardened runtime. Sparkle's helpers keep
     their entitlements. The app gets only `disable-library-validation`, so replaced Qt libraries still load.
     Without `--sign` the bundle is signed ad hoc.
  5. Make a DMG with an Applications link. With `--notarize`, submit it with `notarytool` and staple it.
  6. Ship notices in `Contents/Resources/Licenses`: the cargo-about report, LGPL and GPL texts, the Qt and
     FFmpeg notice, and Sparkle's license.
- **Updates on macOS: Sparkle 2.**
  - The feed is an EdDSA-signed appcast (`SUFeedURL`, `SUPublicEDKey`, from `FM_APPCAST_URL` and
    `FM_SPARKLE_PUBLIC_KEY` at packaging time).
  - Sparkle is loaded with `NSBundle load` and driven through the Objective-C runtime, not linked. The binary
    therefore runs unchanged in development, where the plist has no feed and the updater stays off.
  - Sparkle asks for consent to automatic checks on the second launch. *Check for Updates…* and an automatic
    checkbox are in the About/License dialog.
- **Linux AppImage** (`cargo xtask package-appimage`):
  - linuxdeploy with its Qt plugin (QML import scanning, Wayland plugins).
  - Update information `gh-releases-zsync|owner|repo|latest|FinderMaxxing-*-x86_64.AppImage.zsync` (from
    `FM_RELEASES_REPO`) is embedded. *Check for Updates…* runs `appimageupdatetool` when installed, fetching
    only the changed blocks. Otherwise the dialog links to the releases page.
  - Built on Ubuntu 22.04, so it runs on older glibc versions.
- **Linux Flatpak** (`packaging/linux/flatpak/`):
  - Runtime `org.kde.Platform` 6.8, plus the Qt WebEngine base app for QtPdf. The Rust SDK extension builds
    offline from `flatpak-cargo-generator` sources.
  - Permissions: host file system (it is a file manager), network, Secret Service, and `org.freedesktop.Flatpak`,
    so the terminal runs the user's login shell on the host via `flatpak-spawn --host`.
  - Updates come through Flatpak itself, and the app shows no updater.
- **Release CI** (`.github/workflows/release.yml`, on `v*` tags):
  1. macOS universal: sign with a temporary keychain and notarize.
  2. AppImage plus zsync.
  3. Flatpak bundle.
  4. Generate the appcast with Sparkle's `generate_appcast` and the EdDSA private key.
  5. Publish everything to the public releases repository (`FM_RELEASES_REPO`) on GitHub Releases. The feed is
     `…/releases/latest/download/appcast.xml`.

## Consequences
- The bundle is about 275 MB (123 MB DMG), mostly Qt frameworks that macdeployqt copies whether or not QML uses
  them. Pruning by QML import is a later optimisation.
- Flatpak's `--filesystem=host` and `org.freedesktop.Flatpak` permissions need a justification on Flathub
  review. File managers usually get them.
- In the Flatpak terminal, the foreground process is `flatpak-spawn`, not the shell, so "follow the folder" only
  types `cd` when the terminal is first opened.
- The bundle id and Flatpak id `dev.findermaxxing.FinderMaxxing` are permanent once published. Change them
  before the first release if another domain is wanted.
