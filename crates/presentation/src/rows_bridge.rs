//! Row types stored by the C++ list models, plus the vector edits
//! `EntryListModel` delegates back to Rust (`rust::Vec` has no insert/erase in C++).

#[cxx::bridge]
pub(crate) mod ffi {
    /// One display row. Pre-formatted in Rust so `data()` is a field read.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct EntryRow {
        name: String,
        location: String,
        icon_name: String,
        size_text: String,
        modified_ms: i64,
        has_modified: bool,
        is_container: bool,
        is_hidden: bool,
    }

    /// One running job in the jobs panel.
    #[derive(Debug, Clone, PartialEq)]
    struct JobRow {
        id: i32,
        title: String,
        detail: String,
        /// Completed share in 0..=1; meaningful only when `determinate`.
        fraction: f64,
        determinate: bool,
    }

    extern "Rust" {
        fn rows_insert(rows: &mut Vec<EntryRow>, index: usize, row: EntryRow);
        fn rows_remove(rows: &mut Vec<EntryRow>, index: usize);
        fn rows_replace(rows: &mut Vec<EntryRow>, index: usize, row: EntryRow);
    }

    impl Vec<EntryRow> {}
}

pub(crate) use ffi::{EntryRow, JobRow};

// The C++ model checks bounds before calling these; out-of-range indices
// are ignored rather than panicking across the FFI boundary. Signatures take
// `&mut Vec` because the C++ side holds a `rust::Vec`, not a slice.

fn rows_insert(rows: &mut Vec<EntryRow>, index: usize, row: EntryRow) {
    if index <= rows.len() {
        rows.insert(index, row);
    }
}

fn rows_remove(rows: &mut Vec<EntryRow>, index: usize) {
    if index < rows.len() {
        rows.remove(index);
    }
}

#[allow(clippy::ptr_arg, reason = "cxx exposes the C++-owned rust::Vec only as &mut Vec")]
fn rows_replace(rows: &mut Vec<EntryRow>, index: usize, row: EntryRow) {
    if let Some(slot) = rows.get_mut(index) {
        *slot = row;
    }
}
