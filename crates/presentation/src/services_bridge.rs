//! Hands the Rust service graph to C++: the bootstrap wraps it in a
//! `ServicesHolder` QObject and gives that to the root QML window.

#[cxx::bridge]
pub(crate) mod ffi {
    extern "Rust" {
        type AppServices;
    }

    unsafe extern "C++" {
        include!("presentation/cpp/bootstrap.h");
        include!("presentation/cpp/services_holder.h");

        type ServicesHolder;

        fn services(self: &ServicesHolder) -> &AppServices;

        #[cxx_name = "runApplication"]
        fn run_application(services: Box<AppServices>) -> i32;
    }
}

pub(crate) use ffi::{ServicesHolder, run_application};

use crate::services::AppServices;
