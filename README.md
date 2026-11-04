# FinderMaxxing

A fast, native-feeling file manager for macOS and Linux. Rust + Qt 6 (cxx-qt, QML).
See [docs/architecture.md](docs/architecture.md) and [docs/adr/](docs/adr/).

**Status: milestone 1.** Window, tabs, and a lazily loaded, live-updating local folder view.

## Prerequisites

| | macOS 14+ | Ubuntu 24.04+ |
|---|---|---|
| Rust | stable via rustup (`rust-toolchain.toml`) | same |
| C++ | Xcode command line tools | `sudo apt install build-essential libgl1-mesa-dev libxkbcommon-dev libfontconfig1-dev` |
| Qt 6.8.3 | online installer, or `uvx --from aqtinstall aqt install-qt mac desktop 6.8.3 clang_64 -m qtimageformats -O ~/Qt` | `uvx --from aqtinstall aqt install-qt linux desktop 6.8.3 linux_gcc_64 -m qtimageformats -O ~/Qt` (distro Qt ≥ 6.5 also works: `qt6-base-dev qt6-declarative-dev qml6-module-qtquick-controls qml6-module-qtquick-layouts qt6-image-formats-plugins`) |
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

## Layout

```
crates/
  domain/                 pure entities and rules (StoragePath, FileEntry, SortSpec, VfsError, Capabilities)
  application/            ports (Lister, Watcher, Clock, ProviderFactory) + use cases + in-memory fakes
  vfs-contract-tests/     Liskov contract every provider must pass
  adapter-local-fs/       file:// provider (std::fs + notify)
  adapter-tokio-runtime/  Clock on tokio
  presentation/           cxx-qt controller, C++ list model shim, QML
  app/                    composition root (the `findermaxxing` binary)
  arch-tests/             fails CI if layers point the wrong way
xtask/                    cargo xtask ci | licenses
```
