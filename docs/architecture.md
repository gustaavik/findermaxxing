# Architecture

Dependencies point inward. `crates/arch-tests` fails the build if they don't (ADR 0001).

```mermaid
flowchart RL
    subgraph core[" "]
        domain["domain<br/><i>entities, value objects, rules</i>"]
        application["application<br/><i>use cases + ports</i>"]
    end
    application --> domain

    local["adapter-local-fs<br/><i>Lister, Watcher, Copier, Mover,<br/>Remover, FolderMaker, Trasher, Launcher</i>"] --> application
    tokio_rt["adapter-tokio-runtime<br/><i>Clock</i>"] --> application
    contract["vfs-contract-tests<br/><i>Liskov contract</i>"] --> application
    presentation["presentation<br/><i>cxx-qt bridge, C++ shim, QML</i>"] --> application

    app["app<br/><i>composition root</i>"] --> local
    app --> tokio_rt
    app --> presentation
```

## Ports and their implementations (M1 + M2)

| Port (application) | Implemented by | Proven by |
|---|---|---|
| `Lister` | `adapter_local_fs::LocalFs`, `application::testing::FakeVfs` | `vfs-contract-tests` via `adapter-local-fs/tests/contract.rs` and `vfs-contract-tests/tests/fake_vfs_contract.rs`; local-only cases in `adapter-local-fs/tests/local_fs.rs` |
| `Watcher` | `LocalFs` (notify: FSEvents / inotify), `FakeVfs` | same contract suite (`watcher_reports_create_and_remove`, `watch_missing_dir_is_not_found`) |
| `Copier`, `Mover`, `Remover`, `FolderMaker` | `LocalFs` (clonefile / chunked copy, no-replace rename), `FakeVfs` | `mutation_contract_tests!` (13 cases) for both; byte-copy and cancellation unit tests in `adapter-local-fs/src/mutation.rs` |
| `Trasher` | `LocalTrash` (macOS `NSFileManager`, freedesktop Trash 1.0), `FakeVfs` | `trash_contract_tests!` against the real platform trash and the fake; freedesktop layout unit tests |
| `Launcher` | `LocalFs` (`open` / `xdg-open`), `FakeVfs` | `application/tests/file_operations.rs::open_launches_the_item` |
| `JobObserver`, `ConflictResolver` | `presentation::job_reporting` (Qt thread), `testing::{RecordingObserver, ScriptedResolver}` | use-case tests in `application/tests/file_operations.rs` |
| `Clock` | `adapter_tokio_runtime::TokioClock`, `testing::InstantClock` | `adapter-tokio-runtime` unit test |
| `ProviderFactory` / `MountResolver` | `LocalFsFactory`, `ProviderRegistry` | `application::registry` tests |

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

## Dependency injection without globals
`app` builds `AppServices` (use cases + runtime handle). `bootstrap.cpp` wraps them in a `ServicesHolder` QObject
and passes it to the root window as a required property. Each `FolderView` hands it to its controller.
