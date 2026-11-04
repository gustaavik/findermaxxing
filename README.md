# FinderMaxxing

A fast, native-feeling file manager for macOS and Linux. Rust + Qt 6 (cxx-qt, QML).
See [docs/architecture.md](docs/architecture.md) and [docs/adr/](docs/adr/).

**Status: milestone 5** (5a and 5b).
- M1: window, tabs, and a lazily loaded, live-updating folder view.
- M2: copy, move, rename, new folder, trash with exact restore, permanent delete, and open with the default app. Jobs show progress, ask about conflicts, and can be undone.
- M3: a preview pane (images, PDF, audio/video, highlighted code, archives) plus Quick Look on macOS; search by name, kind, date and size, with saved searches; and bulk rename with a live preview.
- M5b: Google Drive, Dropbox and OneDrive, signed in through the browser (OAuth + PKCE), with Google Docs exported to Office formats. Needs OAuth client IDs: [docs/oauth-setup.md](docs/oauth-setup.md).
- M5a: SFTP, WebDAV and SMB servers; iCloud Drive and other synced cloud folders under Locations; a size-limited cache with *Keep Available Offline*; transfers that resume after a quit or crash; drag and drop with other apps.
- M4: S3 and S3-compatible storage (AWS, R2, B2, Wasabi, MinIO, SeaweedFS) in a unified sidebar with places and favorites. Copy and move stream between providers, transfers that may cost money ask first, and secrets live in the OS keychain.

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

S3 contract tests need a server. CI uses SeaweedFS (MinIO's images are no longer pullable):

```sh
docker run -d --rm --name findermaxxing-s3 -p 127.0.0.1:8333:8333 chrislusf/seaweedfs \
  server -s3 -dir=/data -volume.max=1000 -master.volumeSizeLimitMB=64
cargo test -p adapter-s3 --test s3_contract -- --ignored     # FM_S3_TEST_ENDPOINT, _ACCESS_KEY, _SECRET_KEY override the defaults
cargo test -p adapter-keyring -- --ignored                   # writes and deletes one item in your real keychain
docker stop findermaxxing-s3
```

File-server contract tests (OpenSSH, Apache `mod_dav` and Samba in Docker, as in CI):

```sh
docker run -d --rm --name findermaxxing-sftp -p 127.0.0.1:2222:22 atmoz/sftp fmtest:fmtest-secret:::upload
docker run -d --rm --name findermaxxing-dav -p 127.0.0.1:8090:80 -e AUTH_TYPE=Basic -e USERNAME=fmtest -e PASSWORD=fmtest-secret bytemark/webdav
docker run -d --rm --name findermaxxing-smb -p 127.0.0.1:1445:445 -e USER=fmtest -e PASS=fmtest-secret -e NAME=Data dockurr/samba
cargo test -p adapter-sftp --test sftp_contract -- --ignored
cargo test -p adapter-webdav --test dav_contract -- --ignored
cargo test -p adapter-smb --test smb_contract -- --ignored
docker stop findermaxxing-sftp findermaxxing-dav findermaxxing-smb
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

## Manually testing M4

Start SeaweedFS as above (or use a real bucket), then `cargo run -p app --release`.

- **Add an account:** *Add Cloud Storage…* at the bottom of the sidebar. Pick *MinIO / SeaweedFS*, endpoint `http://127.0.0.1:8333`, access key `fmtest` / `fmtest-secret-key`. A wrong secret or endpoint shows the error and saves nothing.
- **Buckets:** they appear under the account. For a key that can't list buckets (or SeaweedFS without an identity config), right-click the account → *Add Bucket by Name…* and type a name (create it first with `aws --endpoint-url http://127.0.0.1:8333 s3 mb s3://demo`).
- **Upload and download:** copy files in a local folder, paste in the bucket, and the other way round. Paste a file that exists to see the conflict prompt; on S3, *Replace* warns that it's permanent. A file over 8 MiB uploads in parallel parts; cancel it mid-way and nothing is left in the bucket.
- **Cost warning:** copy something over 1 GB to or from the bucket. *Don't ask again for this account* sticks across restarts.
- **No Trash on S3:** `Cmd+Backspace` / `Delete` offers permanent deletion with a confirmation. Undo is not offered afterwards.
- **Undo:** undoing an upload deletes the uploaded objects; undoing a move back from S3 restores both sides.
- **Get Info** (right-click): size, ETag and other facts, editable content type and user metadata, versions if the bucket has versioning, and *Create Link* for a presigned share link (up to 7 days).
- **Open:** double-clicking an S3 file downloads it to `~/Library/Caches/FinderMaxxing/s3` (`~/.cache/FinderMaxxing/s3`) and opens it.
- **Favorites:** right-click any folder, local or S3 → *Add to Sidebar*. Mounted volumes appear under *Locations*, accounts under *Cloud Storage*.
- **Remove the account:** right-click → *Remove Account*. The keychain item goes with it (check *Keychain Access* for `FinderMaxxing`).

## Manually testing M5a

Start the three servers above, then `cargo run -p app --release`.

- **Connect to a server:** *Connect to Server…* at the bottom of the sidebar.
  - **SFTP:** server `127.0.0.1`, port `2222`, folder `upload`, user `fmtest`, password `fmtest-secret`. The first time you'll see the server's key fingerprint; *Trust and Connect* remembers it, and later connections don't ask. Compare it with `ssh-keyscan -p 2222 127.0.0.1 | ssh-keygen -lf -`.
  - **WebDAV:** port `8090`, untick *Use HTTPS* (note the warning), same user and password. For Nextcloud, use your server with path `remote.php/dav/files/<user>`.
  - **SMB:** port `1445`, share `Data`, same user and password. For a NAS, leave the port empty (445).
  - A wrong password or address shows the problem and saves nothing.
- **Your own SSH keys:** *Sign in with: SSH agent* uses whatever `ssh-add -l` lists; *Key file* takes Ed25519 or ECDSA keys. RSA keys work through the agent only (see ADR 0021).
- **Browse and work:** copy, paste, move, rename, new folder and delete work as on the local disk. Servers have no Trash, so deleting asks first.
- **Drag and drop:**
  - Drag files from a server to the Desktop or into a mail: they are downloaded first.
  - Drag from Finder into a server folder, or between two server folders.
  - Within one location a drag moves; across locations it copies. Hold ⌥ to copy or ⌘ to move (Ctrl/Shift on Linux).
  - Drops onto a folder row or a sidebar entry go into that folder.
- **Offline:**
  1. Right-click a server folder → *Keep Available Offline*. A job downloads it.
  2. Stop the container (`docker stop findermaxxing-sftp`).
  3. The folder is still listed and its files still open (read-only).
  4. *Remove Offline Copy* unpins it.
- **Resume:** copy a big file (`mkfile 2g /tmp/big`) to the SFTP server and quit the app mid-way. Restart: the window offers *Resume* (it continues where it stopped) or *Discard* (deletes the partial upload). The same works for S3 and SMB; WebDAV restarts the file.
- **Cloud folders:** iCloud Drive and any Google Drive, OneDrive or Dropbox desktop folders appear under *Locations*.
- **Remove a server:** right-click → *Remove Server…*. The keychain item, cached copies and offline pins go with it.

## Manually testing M5b

Without real accounts, against the local API doubles:

```sh
cargo run -p mock-cloud --bin mock-cloud-server               # prints FM_DEV_CLOUD_MOCK=http://127.0.0.1:<port>
FM_DEV_CLOUD_MOCK=http://127.0.0.1:<port> cargo run -p app    # debug build
```

- *Add Cloud Storage…* → **Google Drive** → *Sign In with Browser…*. With the mock, the sign-in completes at once. Do the same for Dropbox and OneDrive. Each appears under *Cloud Storage* with its user's email.
- **Google Drive:**
  - The two `notes.txt` files show as `notes.txt` and `notes (2).txt`; each opens its own content.
  - `Budget` shows as `Budget.xlsx`; opening exports it.
  - *Move to Trash* works and undo restores.
- Copy, paste, drag and drop between the drives, local folders and servers. Big files resume after a quit, as in M5a.
- *Remove Account…* forgets the sign-in and any offline copies.

With real accounts: register the apps and build with the IDs as described in [docs/oauth-setup.md](docs/oauth-setup.md), then *Sign In with Browser…* opens the provider's own page. Run each adapter's `live_round_trip` test too.

## Layout

```
crates/
  domain/                 pure entities and rules (StoragePath, FileEntry, SortSpec, VfsError, Capabilities)
  application/            ports (Lister, Watcher, Clock, ProviderFactory) + use cases + in-memory fakes
  vfs-contract-tests/     Liskov contract every provider must pass
  adapter-local-fs/       file:// provider: listing, watching, copy/move/remove, streams, platform trash, launcher, volumes
  adapter-preview/        syntax highlighting (syntect) and archive listings (zip, tar)
  adapter-s3/             s3:// provider (aws-sdk-s3): listing, server-side copy, streaming multipart upload, presign
  adapter-keyring/        CredentialStore on the OS keychain (Keychain, Secret Service)
  adapter-file-cache/     size-limited cache of remote files, offline pins and listings (decorates any provider)
  adapter-sftp/           sftp:// provider (russh): host-key trust, agent/key/password, resumable writes
  adapter-webdav/         dav:// provider (reqwest + rustls): Nextcloud, ownCloud, NAS, Apache
  adapter-gdrive/         gdrive:// provider (Drive API v3): duplicates, Docs export, Trash, resumable uploads
  adapter-dropbox/        dropbox:// provider (API v2): upload sessions resumed from Dropbox's offset
  adapter-onedrive/       onedrive:// provider (Microsoft Graph): pre-authorised upload sessions, async copies
  infra-oauth/            shared by the drive adapters: OAuth 2.0 + PKCE loopback sign-in, token refresh, ApiClient
  mock-cloud/             test doubles of the three APIs and an OAuth server (tests and mock-cloud-server only)
  adapter-smb/            smb:// provider (pure-Rust smb2): Windows and NAS shares, resumable writes
  adapter-sqlite-store/   metadata store (SQLite): saved searches, accounts, servers, favorites, cache index, pins, transfer journal
  adapter-tokio-runtime/  Clock on tokio
  presentation/           cxx-qt controllers (folder view, operations, preview, bulk rename, sidebar, item info), C++ models,
                          clipboard, keyboard and Quick Look shims, QML
  app/                    composition root (the `findermaxxing` binary)
  arch-tests/             fails CI if layers point the wrong way
xtask/                    cargo xtask ci | licenses
```
