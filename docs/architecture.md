# Architecture

Dependencies point inward. `crates/arch-tests` fails the build if they don't (ADR 0001).

```mermaid
flowchart RL
    subgraph core[" "]
        domain["domain<br/><i>entities, value objects, rules</i>"]
        application["application<br/><i>use cases + ports</i>"]
    end
    application --> domain

    local["adapter-local-fs<br/><i>Lister, Watcher, Copier, Mover,<br/>Remover, FolderMaker, Trasher, Launcher,<br/>Reader, Writer, VolumeLister, LocalFileOpener</i>"] --> application
    tokio_rt["adapter-tokio-runtime<br/><i>Clock</i>"] --> application
    preview["adapter-preview<br/><i>TextRenderer, ArchiveInspector</i>"] --> application
    store["adapter-sqlite-store<br/><i>SavedSearchStore, AccountStore,<br/>FavoritesStore</i>"] --> application
    s3["adapter-s3<br/><i>Lister, Copier, Mover, Remover, FolderMaker,<br/>Reader, Writer, Materializer, MetadataEditor,<br/>Presigner, VersionLister, S3Connector</i>"] --> application
    keyring["adapter-keyring<br/><i>CredentialStore</i>"] --> application
    contract["vfs-contract-tests<br/><i>Liskov contract</i>"] --> application
    presentation["presentation<br/><i>cxx-qt bridge, C++ shim, QML</i>"] --> application

    app["app<br/><i>composition root</i>"] --> local
    app --> tokio_rt
    app --> preview
    app --> store
    app --> s3
    app --> keyring
    app --> presentation
```

## Ports and their implementations (M1–M4)

| Port (application) | Implemented by | Proven by |
|---|---|---|
| `Lister` | `adapter_local_fs::LocalFs`, `application::testing::FakeVfs` | `vfs-contract-tests` via `adapter-local-fs/tests/contract.rs` and `vfs-contract-tests/tests/fake_vfs_contract.rs`; local-only cases in `adapter-local-fs/tests/local_fs.rs` |
| `Watcher` | `LocalFs` (notify: FSEvents / inotify), `FakeVfs` | same contract suite (`watcher_reports_create_and_remove`, `watch_missing_dir_is_not_found`) |
| `Copier`, `Mover`, `Remover`, `FolderMaker` | `LocalFs` (clonefile / chunked copy, no-replace rename), `FakeVfs` | `mutation_contract_tests!` (13 cases) for both; byte-copy and cancellation unit tests in `adapter-local-fs/src/mutation.rs` |
| `Trasher` | `LocalTrash` (macOS `NSFileManager`, freedesktop Trash 1.0), `FakeVfs` | `trash_contract_tests!` against the real platform trash and the fake; freedesktop layout unit tests |
| `Launcher` | `LocalFs` (`open` / `xdg-open`), `FakeVfs` | `application/tests/file_operations.rs::open_launches_the_item` |
| `JobObserver`, `ConflictResolver` | `presentation::job_reporting` (Qt thread), `testing::{RecordingObserver, ScriptedResolver}` | use-case tests in `application/tests/file_operations.rs` |
| `Materializer` | `LocalFs` (the file itself), `FakeVfs` | `application/tests/browse_and_find.rs::previews_are_routed_by_category` |
| `TextRenderer` | `adapter_preview::SyntectTextRenderer`, `testing::StubTextRenderer` | `adapter-preview/src/text.rs` tests (language detection, binary, truncation) |
| `ArchiveInspector` | `adapter_preview::LocalArchiveInspector`, `testing::StubArchiveInspector` | `adapter-preview/src/archive.rs` tests (tar, tar.gz, zip, unsupported) |
| `SavedSearchStore` | `adapter_sqlite_store::SqliteStore`, `testing::InMemorySavedSearches` | `adapter-sqlite-store/tests/saved_searches.rs` (persistence across reopen, ordering, corrupt rows) |
| `Clock` | `adapter_tokio_runtime::TokioClock`, `testing::InstantClock` | `adapter-tokio-runtime` unit test |
| `ProviderFactory` / `MountResolver` | `LocalFsFactory`, `adapter_s3::S3Service`, `ProviderRegistry` | `application::registry` tests |
| `Lister`, `Copier`, `Mover`, `Remover`, `FolderMaker` on S3 | `adapter_s3::BucketFs` | `adapter-s3/tests/s3_contract.rs` (lister, mutation and stream suites, 29 cases) against SeaweedFS in CI |
| `Reader`, `Writer` | `LocalFs` (hidden partial file + no-replace rename), `BucketFs` (PUT / multipart, `If-None-Match: *`), `FakeVfs` | `stream_contract_tests!` (7 cases) for all three; cross-provider use cases in `application/tests/across_providers.rs` |
| `MetadataEditor`, `Presigner`, `VersionLister` | `BucketFs` | `application/tests/across_providers.rs` (use cases on the fake); manual check against SeaweedFS |
| `S3Connector` | `S3Service`, `testing::FakeS3Connector` | `across_providers.rs` (add/rollback, buckets by name, permission denied) |
| `AccountStore`, `FavoritesStore` | `SqliteStore`, `testing::{InMemoryAccounts, InMemoryFavorites}` | `adapter-sqlite-store/tests/accounts_and_favorites.rs` |
| `CredentialStore` | `adapter_keyring::KeyringStore` (Keychain / Secret Service), `testing::InMemoryKeychain` | `adapter-keyring/tests/os_keychain.rs` (`--ignored`, touches the real keychain) |
| `VolumeLister`, `LocalFileOpener` | `adapter_local_fs::{LocalVolumes, LocalOpener}`, `testing::{FixedVolumes, RecordingOpener}` | `OpenEntry` tests in `across_providers.rs`; volumes checked manually |
| `CostConfirmer` | `presentation::job_reporting::QtCostConfirmer`, `testing::ScriptedCosts`, `operations::AcceptCosts` (undo/redo) | `across_providers.rs` cost cases; `domain::cost` unit tests |

## Runtime flow: opening a folder

1. QML calls `FolderViewController.openLocation(text)`.
2. The controller records history and replaces its `FolderSession`. Dropping the old session cancels it.
3. The session subscribes through `WatchDirectory` and streams `ListDirectory` batches into `DirectoryViewModel`.
4. Sorted snapshots and row edits travel as `ModelUpdate`s through `CxxQtThread::queue` to the Qt thread, then into the C++ `EntryListModel`.

## Runtime flow: a file operation (M2)

1. QML calls `OperationsController.paste(locationId, Copy)`.
2. The controller reads file URLs from the system clipboard and starts a job with a `JobContext`: a throttled `QtJobObserver`, a `QtConflictResolver` and a cancel token.
3. `CopyEntries` scans the sources for totals, resolves name clashes by asking the resolver (with "apply to all"), copies through `Copier`/`FolderMaker`, and records the `Operation` in `OperationHistory`.
4. The watcher reports the new items to every open folder view. The job's end updates the undo/redo labels.

## Runtime flow: a cross-provider copy (M4)

1. Paste in an S3 folder: the controller takes `s3://` locations from the in-app clipboard (or `file://` URLs from the system one).
2. `CopyEntries` scans the sources, builds a `TransferEstimate` and asks `CostPolicy`; if it warns, `QtCostConfirmer` shows `CostDialog.qml` unless the account opted out.
3. Each item is routed: server-side `CopyObject` inside one bucket, otherwise `Reader` → `Writer` streaming (8 MiB multipart parts, 4 in flight).
4. S3 has no watcher, so when the job ends the controller emits `folders_changed` and open views of the affected folders reload.

## Dependency injection without globals
`app` builds `AppServices` (use cases + runtime handle). `bootstrap.cpp` wraps them in a `ServicesHolder` QObject
and passes it to the root window as a required property. Each `FolderView` hands it to its controller.
