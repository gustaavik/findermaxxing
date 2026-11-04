# FinderMaxxing

A fast, native-feeling file manager for macOS and Linux. Rust + Qt 6 (cxx-qt, QML).
See [docs/architecture.md](docs/architecture.md) and [docs/adr/](docs/adr/).

**Status: milestone 3.**
- M1: window, tabs, and a lazily loaded, live-updating folder view.
- M2: copy, move, rename, new folder, trash with exact restore, permanent delete, and open with the default app. Jobs show progress, ask about conflicts, and can be undone.
- M3: a preview pane (images, PDF, audio/video, highlighted code, archives) plus Quick Look on macOS; search by name, kind, date and size, with saved searches; and bulk rename with a live preview.

## Prerequisites

| | macOS 14+ | Ubuntu 24.04+ |
|---|---|---|
| Rust | stable via rustup (`rust-toolchain.toml`) | same |
| C++ | Xcode command line tools | `sudo apt install build-essential libgl1-mesa-dev libxkbcommon-dev libfontconfig1-dev` |
| Qt 6.8.3 | online installer, or `uvx --from aqtinstall aqt install-qt mac desktop 6.8.3 clang_64 -m qtimageformats qtpdf qtmultimedia -O ~/Qt` | `uvx --from aqtinstall aqt install-qt linux desktop 6.8.3 linux_gcc_64 -m qtimageformats qtpdf qtmultimedia -O ~/Qt` (distro Qt ≥ 6.5 also works: `qt6-base-dev qt6-declarative-dev qml6-module-qtquick-controls qml6-module-qtquick-layouts qt6-image-formats-plugins qt6-pdf-dev qml6-module-qtquick-pdf qt6-multimedia-dev qml6-module-qtmultimedia`) |
| CI tools | `cargo install --locked cargo-deny` and `cargo install --locked cargo-about --features cli` | same |

Point cxx-qt at Qt (add it to your shell profile):

```sh
export QMAKE=~/Qt/6.8.3/macos/bin/qmake      # Linux: ~/Qt/6.8.3/gcc_64/bin/qmake (or /usr/bin/qmake6)
```

## Build and run

```sh
cargo run -p app                 # debug
cargo run -p app --release       # what you should judge performance on
```

Without `QMAKE`, plain `cargo build` / `cargo test` only builds the Qt-free crates (`default-members`).

## Test

```sh
cargo xtask ci                                    # fmt, clippy -D warnings, tests, cargo-deny (Qt crates too if QMAKE is set)
cargo test --release -p adapter-local-fs --test local_fs -- --ignored large_dir --nocapture   # 100k-file benchmark
cargo xtask licenses                              # target/third-party-licenses.html
```

## Manually testing M1

```sh
mkdir -p /tmp/big && (cd /tmp/big && seq -f "file-%06g.txt" 100000 | xargs touch)
cargo run -p app --release
```

- Type `/tmp/big` in the location field (`Cmd/Ctrl+L`) and press Return. Rows should appear immediately and stay smooth while scrolling.
- In a terminal, run `touch /tmp/big/zzz.txt` and `rm /tmp/big/file-000001.txt`. The view updates within about 100 ms.
- Click the column headers to sort (natural order, folders first). Click again to reverse.
- Keys: `Cmd/Ctrl+T` new tab · `Cmd/Ctrl+W` close · `Ctrl+Tab` next tab · `Cmd/Ctrl+↑` or `Alt+↑` enclosing folder ·
  `Cmd+[` / `Cmd+]` (`Alt+←/→` on Linux) back/forward · `Return` or `Cmd/Ctrl+↓` open folder · `Cmd/Ctrl+Shift+.` hidden files · `Cmd+R` / `F5` reload.
- Screen reader: VoiceOver/Orca read row names, buttons and column headers.

## Manually testing M2

```sh
rm -rf /tmp/m2 && mkdir -p /tmp/m2/src/nested /tmp/m2/dst && echo one > /tmp/m2/src/a.txt && echo two > /tmp/m2/dst/a.txt
dd if=/dev/zero of=/tmp/m2/src/big.bin bs=1m count=2000      # 2 GB; on APFS the copy is an instant clone
cargo run -p app --release
```

- **Selection:** click; `Cmd/Ctrl`+click toggles; `Shift`+click or `Shift`+arrows extends; `Cmd/Ctrl+A` selects all; `Esc` clears.
- **Copy and paste:** select `a.txt` in `/tmp/m2/src` and press `Cmd/Ctrl+C`. Open `/tmp/m2/dst` and press `Cmd/Ctrl+V`. The conflict dialog appears. Try Replace, Keep Both (`a 2.txt`), Skip and "Apply to all". Then press `Cmd/Ctrl+Z`: a Replace brings `two` back.
- **Interop:** copy in Finder or Nautilus and paste here, or the other way round.
- **Move Here:** `Cmd+Option+V` / `Ctrl+Alt+V`. Undo moves the items back.
- **Progress:** to see the bar and cancel button on a big copy, copy `big.bin` to a USB stick or a non-APFS volume. On APFS the clone is instant.
- **Rename:** `Return` (macOS) or `F2`. The name is preselected without its extension. `Esc` cancels.
- **New folder:** `Cmd/Ctrl+Shift+N` creates "untitled folder" ("New Folder" on Linux) and starts renaming it.
- **Trash:** `Cmd+Backspace` (macOS) or `Delete`. The item shows up in Finder's Trash or `~/.local/share/Trash`, and undo puts it back.
- **Delete immediately:** `Cmd+Option+Backspace` / `Shift+Delete`, with a confirmation. This can't be undone.
- **Open:** double-click or `Cmd/Ctrl+Down` on a file opens it in its default app.
- **Context menu:** right-click on items or on empty space.

## Manually testing M3

- **Preview pane:** the *Preview* button or `Cmd/Ctrl+Shift+P`. Arrow through a folder of photos, PDFs, code, zips and audio or video; the pane follows the current item.
- **Quick Look (macOS):** select items and press `Space` (Space again closes). On Linux, Space toggles the pane.
- **Search:** `Cmd/Ctrl+F`, then type. Results from all subfolders stream in, with a Folder column. Narrow them with Kind, Date and Size. `Esc` or *Done* returns to the folder. Search a big tree such as `~/Documents` to watch the folder counter.
- **Saved searches:** *Save…* names the current search; *Saved ▾* runs it again from anywhere, or deletes it. Saved searches survive restarts (`~/Library/Application Support/FinderMaxxing/metadata.sqlite3`).
- **Bulk rename:** select several items and press `Return` (macOS) or `F2`, or right-click and choose *Rename N Items…*.
  - Try Replace with regex `(\d{4})-(\d{2})-(\d{2})` → `$3.$2.$1`, or Format "Trip", start 1, 2 digits.
  - Problems show in red and those items are skipped.
  - `Cmd/Ctrl+Z` undoes the whole batch.

## Layout

```
crates/
  domain/                 pure entities and rules (StoragePath, FileEntry, SortSpec, VfsError, Capabilities)
  application/            ports (Lister, Watcher, Clock, ProviderFactory) + use cases + in-memory fakes
  vfs-contract-tests/     Liskov contract every provider must pass
  adapter-local-fs/       file:// provider: listing, watching, copy/move/remove, platform trash, launcher
  adapter-preview/        syntax highlighting (syntect) and archive listings (zip, tar)
  adapter-sqlite-store/   metadata store (SQLite): saved searches
  adapter-tokio-runtime/  Clock on tokio
  presentation/           cxx-qt controllers (folder view, operations, preview, bulk rename), C++ models,
                          clipboard and Quick Look shims, QML
  app/                    composition root (the `findermaxxing` binary)
  arch-tests/             fails CI if layers point the wrong way
xtask/                    cargo xtask ci | licenses
```
