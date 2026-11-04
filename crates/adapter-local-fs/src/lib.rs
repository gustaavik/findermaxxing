//! `file://` provider.
//!
//! Implements `Lister`, `Watcher`, `Copier`, `Mover`, `Remover`,
//! `FolderMaker`, `Launcher` and (via [`LocalTrash`]) `Trasher`. Proven by the
//! shared contract suites in `tests/contract.rs` plus local-only cases in
//! `tests/local_fs.rs`. Unix only (macOS, Linux).

mod entry;
mod error;
mod factory;
mod launcher;
mod lister;
mod mutation;
mod native_path;
mod sys;
mod trash;
mod watcher;

pub use factory::LocalFsFactory;
pub use lister::LocalFs;
pub use native_path::{LOCAL_SCHEME, storage_path_from_native};
pub use trash::{FreedesktopTrash, LocalTrash};
