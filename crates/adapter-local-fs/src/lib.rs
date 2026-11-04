//! `file://` provider.
//!
//! Implements the `Lister` and `Watcher` ports. Proven by the shared contract
//! suite in `tests/contract.rs` plus local-only cases in `tests/local_fs.rs`.

mod entry;
mod error;
mod factory;
mod lister;
mod native_path;
mod watcher;

pub use factory::LocalFsFactory;
pub use lister::LocalFs;
pub use native_path::{LOCAL_SCHEME, storage_path_from_native};
