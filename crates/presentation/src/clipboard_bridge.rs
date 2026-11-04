//! System clipboard access (C++ shim over `QClipboard`).

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("presentation/cpp/clipboard.h");

        #[cxx_name = "setClipboardUrls"]
        fn set_clipboard_urls(urls: &Vec<String>);

        #[cxx_name = "clipboardUrls"]
        fn clipboard_urls() -> Vec<String>;
    }
}

pub(crate) use ffi::{clipboard_urls, set_clipboard_urls};
