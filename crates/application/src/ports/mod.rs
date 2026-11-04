//! Ports: small, role-based traits implemented by adapters.
//!
//! Each role has a static trait (`Lister`, native `async`/`impl Stream`) for
//! adapters to implement, and an object-safe `Dyn*` twin with a blanket impl
//! so the registry can store heterogeneous providers behind `Arc<dyn …>`.

mod clock;
mod job;
mod lister;
mod mount;
mod mutation;
mod watcher;

pub use clock::Clock;
pub use job::{ConflictResolver, JobObserver};
pub use lister::{DynLister, Lister};
pub use mount::{MountHandle, MountHandleBuilder, MountResolver, ProviderFactory};
pub use mutation::{
    Copier, DynCopier, DynFolderMaker, DynLauncher, DynMover, DynRemover, DynTrasher, FolderMaker, Launcher, Mover,
    ProgressCallback, Remover, Trasher,
};
pub use watcher::{DynWatcher, Watcher};
