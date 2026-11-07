//! `TerminalController`: the terminal pane. Owns one shell session, pushes
//! its screen into the C++ `TerminalView` (colours resolved for the current
//! theme), and turns the view's key, text, mouse and wheel signals into
//! session calls. cxx-qt can't subclass `QQuickPaintedItem`, so drawing is
//! the C++ item's job (ADR 0028).

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use application::ports::{
    CellPoint, CursorShape, KeyModifiers, TerminalColor, TerminalKey, TerminalScreen, TerminalSession, TerminalSize,
    TextStyle,
};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use domain::StoragePath;

use crate::clipboard_bridge::{clipboard_text, set_clipboard_text};
use crate::rows_bridge::{TerminalCursor, TerminalRun};
use crate::services::AppServices;
use crate::terminal_palette::Palette;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("presentation/src/rows_bridge.cxx.h");
        type TerminalRun = crate::rows_bridge::TerminalRun;
        type TerminalCursor = crate::rows_bridge::TerminalCursor;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    unsafe extern "C++Qt" {
        include!("presentation/cpp/terminal_view.h");
        #[qobject]
        type TerminalView;

        #[cxx_name = "setFrame"]
        fn set_frame(self: Pin<&mut TerminalView>, runs: Vec<TerminalRun>, cursor: TerminalCursor, background: u32);
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(*mut TerminalView, view)]
        #[qproperty(bool, dark)]
        #[qproperty(bool, running)]
        #[qproperty(QString, title)]
        #[qproperty(QString, error_message)]
        type TerminalController = super::TerminalControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        /// Starts a shell in `location` (home for folders not on this machine).
        #[qinvokable]
        fn open(self: Pin<&mut TerminalController>, location: &QString, columns: i32, rows: i32);
        /// `cd`s to `location` if the shell is idle at its prompt.
        #[qinvokable]
        fn follow(self: Pin<&mut TerminalController>, location: &QString);
        #[qinvokable]
        fn key(self: Pin<&mut TerminalController>, key: i32, modifiers: i32, text: &QString);
        #[qinvokable]
        fn type_text(self: Pin<&mut TerminalController>, text: &QString);
        #[qinvokable]
        fn resize(self: Pin<&mut TerminalController>, columns: i32, rows: i32);
        #[qinvokable]
        fn scroll(self: Pin<&mut TerminalController>, lines: i32);
        #[qinvokable]
        fn select(self: Pin<&mut TerminalController>, from_row: i32, from_column: i32, to_row: i32, to_column: i32);
        #[qinvokable]
        fn clear_selection(self: Pin<&mut TerminalController>);
        #[qinvokable]
        fn copy(self: Pin<&mut TerminalController>);
        #[qinvokable]
        fn paste(self: Pin<&mut TerminalController>);
        #[qinvokable]
        fn close(self: Pin<&mut TerminalController>);
    }

    impl cxx_qt::Threading for TerminalController {}
}

use qobject::{ServicesHolder, TerminalView};

/// `Qt::Key` values (qnamespace.h) of the keys that aren't plain text.
mod qt_key {
    pub const ESCAPE: i32 = 0x0100_0000;
    pub const TAB: i32 = 0x0100_0001;
    pub const BACKTAB: i32 = 0x0100_0002;
    pub const BACKSPACE: i32 = 0x0100_0003;
    pub const RETURN: i32 = 0x0100_0004;
    pub const ENTER: i32 = 0x0100_0005;
    pub const INSERT: i32 = 0x0100_0006;
    pub const DELETE: i32 = 0x0100_0007;
    pub const HOME: i32 = 0x0100_0010;
    pub const END: i32 = 0x0100_0011;
    pub const LEFT: i32 = 0x0100_0012;
    pub const UP: i32 = 0x0100_0013;
    pub const RIGHT: i32 = 0x0100_0014;
    pub const DOWN: i32 = 0x0100_0015;
    pub const PAGE_UP: i32 = 0x0100_0016;
    pub const PAGE_DOWN: i32 = 0x0100_0017;
    pub const F1: i32 = 0x0100_0030;
    pub const F12: i32 = 0x0100_003b;
    pub const LETTER_A: i32 = 0x41;
    pub const LETTER_Z: i32 = 0x5a;
}

/// `Qt::KeyboardModifier` bits.
mod qt_modifier {
    pub const SHIFT: i32 = 0x0200_0000;
    pub const CONTROL: i32 = 0x0400_0000;
    pub const ALT: i32 = 0x0800_0000;
    pub const META: i32 = 0x1000_0000;
}

pub struct TerminalControllerRust {
    services: *mut ServicesHolder,
    view: *mut TerminalView,
    dark: bool,
    running: bool,
    title: QString,
    error_message: QString,

    session: Option<Arc<dyn TerminalSession>>,
    /// A redraw is queued; further changes until then ride along.
    redraw_queued: Arc<AtomicBool>,
}

impl Default for TerminalControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            view: std::ptr::null_mut(),
            dark: false,
            running: false,
            title: QString::default(),
            error_message: QString::default(),
            session: None,
            redraw_queued: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl qobject::TerminalController {
    pub fn open(mut self: Pin<&mut Self>, location: &QString, columns: i32, rows: i32) {
        let Some(services) = self.app_services() else { return };
        let folder = StoragePath::decode(&location.to_string()).unwrap_or_else(|_| services.home.clone());
        let thread = self.qt_thread();
        let queued = Arc::clone(&self.redraw_queued);
        let changed = Arc::new(move || {
            if !queued.swap(true, Ordering::AcqRel) {
                let _ = thread.queue(|controller| controller.redraw());
            }
        });
        match services.utilities.terminal.execute(&folder, size(columns, rows), changed) {
            Ok(session) => {
                self.as_mut().rust_mut().session = Some(session);
                self.as_mut().set_running(true);
                self.as_mut().set_error_message(QString::default());
                self.redraw();
            }
            Err(error) => self.set_error_message(QString::from(&error.to_string())),
        }
    }

    pub fn follow(self: Pin<&mut Self>, location: &QString) {
        let (Some(session), Ok(folder)) = (self.session.clone(), StoragePath::decode(&location.to_string())) else {
            return;
        };
        if let Some(native) = application::use_cases::local_folder(&folder) {
            session.follow(&native);
        }
    }

    pub fn key(self: Pin<&mut Self>, key: i32, modifiers: i32, text: &QString) {
        let Some(session) = self.session.clone() else { return };
        for (key, modifiers) in translate_key(key, modifiers, &text.to_string()) {
            session.key(key, modifiers);
        }
    }

    pub fn type_text(self: Pin<&mut Self>, text: &QString) {
        if let Some(session) = &self.session {
            session.paste(&text.to_string());
        }
    }

    pub fn resize(self: Pin<&mut Self>, columns: i32, rows: i32) {
        if let Some(session) = &self.session {
            session.resize(size(columns, rows));
        }
    }

    pub fn scroll(self: Pin<&mut Self>, lines: i32) {
        if let Some(session) = &self.session {
            session.scroll(lines);
        }
    }

    pub fn select(self: Pin<&mut Self>, from_row: i32, from_column: i32, to_row: i32, to_column: i32) {
        let cell = |row: i32, column: i32| CellPoint {
            row: u16::try_from(row).unwrap_or(0),
            column: u16::try_from(column).unwrap_or(0),
        };
        let (from, to) = (cell(from_row, from_column), cell(to_row, to_column));
        // The session wants them in reading order.
        let (from, to) = if (to.row, to.column) < (from.row, from.column) { (to, from) } else { (from, to) };
        if let Some(session) = &self.session {
            session.select(from, to);
        }
    }

    pub fn clear_selection(self: Pin<&mut Self>) {
        if let Some(session) = &self.session {
            session.clear_selection();
        }
    }

    pub fn copy(self: Pin<&mut Self>) {
        if let Some(text) = self.session.as_ref().and_then(|s| s.selected_text()) {
            set_clipboard_text(&text);
        }
    }

    pub fn paste(self: Pin<&mut Self>) {
        let text = clipboard_text();
        if let (Some(session), false) = (&self.session, text.is_empty()) {
            session.paste(&text);
        }
    }

    pub fn close(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().session = None;
        self.set_running(false);
    }

    fn redraw(mut self: Pin<&mut Self>) {
        self.redraw_queued.store(false, Ordering::Release);
        let Some(session) = self.session.clone() else { return };
        let screen = session.screen();
        let running = session.is_running();
        if self.running != running {
            self.as_mut().set_running(running);
        }
        if self.title.to_string() != screen.title {
            self.as_mut().set_title(QString::from(&screen.title));
        }
        let palette = Palette::for_theme(self.dark);
        let (runs, cursor) = frame(&screen, &palette);
        // SAFETY: QML sets `view` to an item that outlives this controller
        // (both live in the same pane); null until then.
        if let Some(view) = unsafe { self.view().as_mut() } {
            // SAFETY: the item isn't moved while pinned here; Qt owns it.
            let view = unsafe { Pin::new_unchecked(view) };
            view.set_frame(runs, cursor, palette.background());
        }
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }
}

fn size(columns: i32, rows: i32) -> TerminalSize {
    TerminalSize { columns: u16::try_from(columns).unwrap_or(1).max(1), rows: u16::try_from(rows).unwrap_or(1).max(1) }
}

/// The screen as draw-ready runs, colours resolved with `palette`.
fn frame(screen: &TerminalScreen, palette: &Palette) -> (Vec<TerminalRun>, TerminalCursor) {
    let mut runs = Vec::new();
    for (row, cells) in (0_u16..).zip(&screen.rows) {
        for run in cells {
            let selected = run.style.has(TextStyle::SELECTED);
            let background = if selected { palette.selection() } else { palette.resolve(run.background) };
            let blank = run.text.trim().is_empty();
            if blank && background == palette.background() {
                continue;
            }
            runs.push(TerminalRun {
                row,
                column: run.column,
                cells: run.cells,
                text: run.text.clone(),
                foreground: palette.resolve(run.foreground),
                background,
                style: run.style.0 & !TextStyle::SELECTED,
            });
        }
    }
    let shape = match screen.cursor {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Bar => 2,
        CursorShape::Hidden => 3,
    };
    let cursor = TerminalCursor {
        row: screen.cursor_row,
        column: screen.cursor_column,
        shape: if screen.scrolled_back > 0 { 3 } else { shape },
        color: palette.resolve(TerminalColor::Foreground),
    };
    (runs, cursor)
}

/// On macOS Qt reports ⌘ as Control and ⌃ as Meta; the terminal's Control
/// is the ⌃ key there. ⌥ composes characters on macOS (å, ø, @ on some
/// layouts), so it only prefixes ESC on other systems.
fn terminal_modifiers(qt: i32) -> KeyModifiers {
    let control = if cfg!(target_os = "macos") { qt_modifier::META } else { qt_modifier::CONTROL };
    let alt_is_meta = !cfg!(target_os = "macos");
    let mut bits = 0;
    if qt & qt_modifier::SHIFT != 0 {
        bits |= KeyModifiers::SHIFT;
    }
    if qt & control != 0 {
        bits |= KeyModifiers::CONTROL;
    }
    if alt_is_meta && qt & qt_modifier::ALT != 0 {
        bits |= KeyModifiers::ALT;
    }
    KeyModifiers(bits)
}

/// A Qt key press as terminal keys (text may hold several characters).
fn translate_key(key: i32, modifiers: i32, text: &str) -> Vec<(TerminalKey, KeyModifiers)> {
    let modifiers = terminal_modifiers(modifiers);
    let named = match key {
        qt_key::ESCAPE => Some(TerminalKey::Escape),
        qt_key::TAB => Some(TerminalKey::Tab),
        qt_key::BACKTAB => return vec![(TerminalKey::Tab, KeyModifiers(modifiers.0 | KeyModifiers::SHIFT))],
        qt_key::BACKSPACE => Some(TerminalKey::Backspace),
        qt_key::RETURN | qt_key::ENTER => Some(TerminalKey::Enter),
        qt_key::INSERT => Some(TerminalKey::Insert),
        qt_key::DELETE => Some(TerminalKey::Delete),
        qt_key::HOME => Some(TerminalKey::Home),
        qt_key::END => Some(TerminalKey::End),
        qt_key::LEFT => Some(TerminalKey::Left),
        qt_key::UP => Some(TerminalKey::Up),
        qt_key::RIGHT => Some(TerminalKey::Right),
        qt_key::DOWN => Some(TerminalKey::Down),
        qt_key::PAGE_UP => Some(TerminalKey::PageUp),
        qt_key::PAGE_DOWN => Some(TerminalKey::PageDown),
        qt_key::F1..=qt_key::F12 => u8::try_from(key - qt_key::F1 + 1).ok().map(TerminalKey::Function),
        _ => None,
    };
    if let Some(named) = named {
        return vec![(named, modifiers)];
    }
    if modifiers.has(KeyModifiers::CONTROL) && (qt_key::LETTER_A..=qt_key::LETTER_Z).contains(&key) {
        // The letter itself, not the control character Qt may put in `text`.
        let letter = u8::try_from(key).map(|b| char::from(b.to_ascii_lowercase())).unwrap_or('a');
        return vec![(TerminalKey::Char(letter), modifiers)];
    }
    // Shift is already in the text ("A", "!").
    let text_modifiers = KeyModifiers(modifiers.0 & !KeyModifiers::SHIFT);
    text.chars().filter(|c| !c.is_control()).map(|c| (TerminalKey::Char(c), text_modifiers)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control() -> i32 {
        if cfg!(target_os = "macos") { qt_modifier::META } else { qt_modifier::CONTROL }
    }

    #[test]
    fn named_keys_letters_with_control_and_text() {
        assert_eq!(translate_key(qt_key::UP, 0, ""), [(TerminalKey::Up, KeyModifiers(0))]);
        assert_eq!(
            translate_key(0x43, control(), "\u{3}"),
            [(TerminalKey::Char('c'), KeyModifiers(KeyModifiers::CONTROL))]
        );
        assert_eq!(translate_key(0x41, qt_modifier::SHIFT, "A"), [(TerminalKey::Char('A'), KeyModifiers(0))]);
        assert_eq!(translate_key(0, 0, "é"), [(TerminalKey::Char('é'), KeyModifiers(0))]);
        assert_eq!(translate_key(qt_key::F1 + 4, 0, ""), [(TerminalKey::Function(5), KeyModifiers(0))]);
        assert!(translate_key(0x0100_0020, qt_modifier::SHIFT, "").is_empty(), "a lone modifier sends nothing");
    }

    #[test]
    fn frames_skip_blank_default_cells_and_resolve_colours() {
        let palette = Palette::for_theme(true);
        let run = |text: &str, background| application::ports::StyledRun {
            column: 0,
            text: text.to_owned(),
            cells: 1,
            foreground: TerminalColor::Indexed(1),
            background,
            style: TextStyle(TextStyle::BOLD),
        };
        let screen = TerminalScreen {
            size: size(10, 2),
            rows: vec![vec![run(" ", TerminalColor::Background)], vec![run("x", TerminalColor::Background)]],
            cursor_row: 1,
            cursor_column: 1,
            cursor: CursorShape::Bar,
            title: String::new(),
            scrolled_back: 0,
        };
        let (runs, cursor) = frame(&screen, &palette);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].row, 1);
        assert_eq!(runs[0].foreground, palette.resolve(TerminalColor::Indexed(1)));
        assert_eq!(cursor.shape, 2);
    }
}
