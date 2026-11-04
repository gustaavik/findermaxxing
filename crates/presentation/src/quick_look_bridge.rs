//! macOS Quick Look (Obj-C++ shim; a no-op elsewhere).

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("presentation/cpp/quick_look.h");

        #[cxx_name = "toggleQuickLook"]
        fn toggle_quick_look(file_urls: &Vec<String>) -> bool;

        #[cxx_name = "refreshQuickLook"]
        fn refresh_quick_look(file_urls: &Vec<String>);
    }
}

pub(crate) use ffi::{refresh_quick_look, toggle_quick_look};
