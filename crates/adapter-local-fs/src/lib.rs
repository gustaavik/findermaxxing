//! `file://` provider.
//!
//! Implements `Lister`, `Watcher`, `Copier`, `Mover`, `Remover`,
//! `FolderMaker`, `Launcher`, `Materializer`, `Reader`, `Writer` and (via
//! [`LocalTrash`]) `Trasher`; plus [`LocalOpener`] and [`LocalVolumes`]. Proven by the
//! shared contract suites in `tests/contract.rs` plus local-only cases in
//! `tests/local_fs.rs`. Unix only (macOS, Linux).

mod entry;
mod error;
mod factory;
pub(crate) mod launcher;
mod lister;
mod materializer;
mod mutation;
mod native_path;
mod stream;
mod sys;
mod trash;
mod volumes;
mod watcher;

pub use factory::LocalFsFactory;
pub use launcher::LocalOpener;
pub use lister::LocalFs;
pub use native_path::{LOCAL_SCHEME, storage_path_from_native};
pub use trash::{FreedesktopTrash, LocalTrash};
pub use volumes::LocalVolumes;
