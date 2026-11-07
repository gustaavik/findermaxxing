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
        /// Where a search result lives, relative to the search scope; empty when browsing.
        folder_text: String,
    }

    /// Equally styled terminal cells; colours resolved to ARGB in Rust.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TerminalRun {
        row: u16,
        column: u16,
        cells: u16,
        text: String,
        foreground: u32,
        background: u32,
        /// `TextStyle` bits (bold, italic, underline, dim, strikeout).
        style: u8,
    }

    /// Where and how the terminal cursor is drawn.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct TerminalCursor {
        row: u16,
        column: u16,
        /// 0 block, 1 underline, 2 bar, 3 hidden.
        shape: u8,
        color: u32,
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

    /// One sidebar line: a heading, a place, or an account with its buckets.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SidebarRow {
        /// A `SidebarController::RowKind` value.
        kind: i32,
        label: String,
        /// Secondary text (an account's connection problem); usually empty.
        detail: String,
        /// Lossless location to open; empty for headings and accounts.
        location: String,
        icon_name: String,
        /// The S3 account an account or bucket row belongs to; 0 otherwise.
        account_id: i64,
    }

    extern "Rust" {
        fn rows_insert(rows: &mut Vec<EntryRow>, index: usize, row: EntryRow);
        fn rows_remove(rows: &mut Vec<EntryRow>, index: usize);
        fn rows_replace(rows: &mut Vec<EntryRow>, index: usize, row: EntryRow);
    }

    impl Vec<EntryRow> {}
    impl Vec<SidebarRow> {}
    impl Vec<TerminalRun> {}
}

pub(crate) use ffi::{EntryRow, JobRow, SidebarRow, TerminalCursor, TerminalRun};

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
