//! Use cases and ports. Depends only on `domain` and runtime-agnostic
//! `futures` traits; concrete adapters are injected by the composition root.

pub mod cancel;
pub mod ports;
pub mod registry;
pub mod use_cases;

#[cfg(feature = "testing")]
pub mod testing;

pub use cancel::{CancelToken, CancellationSource};
pub use registry::{ProviderRegistry, RegistryError};
