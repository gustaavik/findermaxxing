//! Composition root. Builds the runtime, registers providers, constructs use
//! cases with their ports, and hands the result to the presentation layer.
//! No other crate names concrete adapters.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use adapter_local_fs::{LocalFs, LocalFsFactory, LocalTrash, storage_path_from_native};
use adapter_tokio_runtime::TokioClock;
use application::ProviderRegistry;
use application::ports::MountResolver;
use application::use_cases::{CoalesceWindow, ListDirectory, WatchDirectory};
use presentation::{AppServices, FileOperations};
use tokio::runtime::Handle;

const HOME_VARIABLE: &str = "HOME";
const FALLBACK_HOME: &str = "/";
const RUNTIME_THREAD_NAME: &str = "findermaxxing-worker";

#[derive(Debug, thiserror::Error)]
enum StartupError {
    #[error("could not start the async runtime: {0}")]
    Runtime(#[from] std::io::Error),
    #[error("could not register storage providers: {0}")]
    Registry(#[from] application::RegistryError),
    #[error("invalid built-in path: {0}")]
    Path(#[from] domain::PathError),
    #[error("could not resolve the home folder: {0}")]
    Home(#[from] domain::VfsError),
}

fn main() -> ExitCode {
    match start() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX)),
        Err(error) => {
            eprintln!("FinderMaxxing failed to start: {error}");
            ExitCode::FAILURE
        }
    }
}

fn start() -> Result<i32, StartupError> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name(RUNTIME_THREAD_NAME).build()?;
    let registry = build_registry(runtime.handle().clone())?;
    let services = AppServices::new(
        Arc::new(ListDirectory::new(registry.clone())),
        Arc::new(WatchDirectory::new(registry.clone(), Arc::new(TokioClock), CoalesceWindow::default())),
        FileOperations::new(&registry),
        runtime.handle().clone(),
        storage_path_from_native(&home_dir())?,
    );
    let exit_code = presentation::run(services);
    // Background tasks are cancelled with their sessions; don't wait on stragglers.
    runtime.shutdown_background();
    Ok(exit_code)
}

/// Every provider is registered here and nowhere else (Open/Closed).
fn build_registry(runtime: Handle) -> Result<Arc<dyn MountResolver>, StartupError> {
    let local = LocalFsFactory::new(LocalFs::new(runtime.clone()), LocalTrash::platform_default(runtime))?;
    Ok(Arc::new(ProviderRegistry::default().with_provider(Arc::new(local))?))
}

fn home_dir() -> PathBuf {
    std::env::var_os(HOME_VARIABLE).map_or_else(|| PathBuf::from(FALLBACK_HOME), PathBuf::from)
}
