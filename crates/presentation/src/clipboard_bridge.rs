//! System clipboard access (C++ shim over `QClipboard`).

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("presentation/cpp/clipboard.h");

        #[cxx_name = "setClipboardUrls"]
        fn set_clipboard_urls(urls: &Vec<String>);

        #[cxx_name = "clipboardUrls"]
        fn clipboard_urls() -> Vec<String>;

        #[cxx_name = "setClipboardText"]
        fn set_clipboard_text(text: &str);

        #[cxx_name = "clipboardText"]
        fn clipboard_text() -> String;
    }
}

pub(crate) use ffi::{clipboard_text, clipboard_urls, set_clipboard_text, set_clipboard_urls};
