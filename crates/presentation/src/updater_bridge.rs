//! Sparkle on macOS (Obj-C++ shim; a no-op elsewhere).

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("presentation/cpp/updater.h");

        #[cxx_name = "startSparkle"]
        fn start_sparkle() -> bool;

        #[cxx_name = "checkForUpdatesNow"]
        fn check_for_updates_now();

        #[cxx_name = "automaticUpdateChecks"]
        fn automatic_update_checks() -> bool;

        #[cxx_name = "setAutomaticUpdateChecks"]
        fn set_automatic_update_checks(enabled: bool);
    }
}

pub(crate) use ffi::{automatic_update_checks, check_for_updates_now, set_automatic_update_checks, start_sparkle};
