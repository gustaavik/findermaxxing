//! Presentation layer: Qt bridge objects, view-models and QML.
//!
//! Bridge objects stay thin: they translate UI events into use-case calls and
//! use-case output into view-model updates. Everything testable without Qt
//! (`view_model`, `history`, `row_format`, `error_messages`) is plain Rust.

mod bulk_rename_controller;
mod clipboard_bridge;
mod error_messages;
mod file_urls;
mod folder_view_controller;
mod history;
mod input_bridge;
mod item_details_controller;
mod job_reporting;
mod job_text;
mod operations_controller;
mod preview_controller;
mod quick_look_bridge;
mod row_format;
mod rows_bridge;
mod search_options;
mod services;
mod services_bridge;
mod session;
mod sidebar_controller;
mod view_model;

pub use services::{
    AppServices, CloudDrives, FileOperations, ItemDetails, Offline, Places, RemoteAccounts, SavedSearches,
    ServerAccounts,
};

/// Starts the Qt event loop and blocks until the last window closes.
/// Returns the process exit code.
#[must_use]
pub fn run(services: AppServices) -> i32 {
    cxx_qt::init_crate!(cxx_qt_lib);
    cxx_qt::init_crate!(presentation);
    cxx_qt::init_qml_module!("FinderMaxxing");
    services_bridge::run_application(Box::new(services))
}
