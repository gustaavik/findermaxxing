//! Ports: small, role-based traits implemented by adapters.
//!
//! Each role has a static trait (`Lister`, native `async`/`impl Stream`) for
//! adapters to implement, and an object-safe `Dyn*` twin with a blanket impl
//! so the registry can store heterogeneous providers behind `Arc<dyn …>`.

mod clock;
mod lister;
mod mount;
mod watcher;

pub use clock::Clock;
pub use lister::{DynLister, Lister};
pub use mount::{MountHandle, MountHandleBuilder, MountResolver, ProviderFactory};
pub use watcher::{DynWatcher, Watcher};
