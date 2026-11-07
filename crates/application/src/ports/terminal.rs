//! The built-in terminal (ADR 0028): a shell on a pseudo-terminal, emulated
//! by an adapter, drawn by the UI from [`TerminalScreen`] snapshots.

use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalColor {
    /// The theme's text colour.
    Foreground,
    /// The theme's background.
    Background,
    /// The 256-colour palette (0–15 follow the theme).
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Text attributes, as bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextStyle(pub u8);

impl TextStyle {
    pub const BOLD: u8 = 1;
    pub const ITALIC: u8 = 1 << 1;
    pub const UNDERLINE: u8 = 1 << 2;
    pub const DIM: u8 = 1 << 3;
    pub const STRIKEOUT: u8 = 1 << 4;
    /// Part of the mouse selection.
    pub const SELECTED: u8 = 1 << 5;

    #[must_use]
    pub fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

/// Consecutive cells of one row with the same look. Inverse video is
/// already applied to the colours; the selection is a style bit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledRun {
    pub column: u16,
    /// One `char` per cell, except that wide characters take two cells.
    pub text: String,
    pub cells: u16,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub style: TextStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalScreen {
    pub size: TerminalSize,
    pub rows: Vec<Vec<StyledRun>>,
    pub cursor_row: u16,
    pub cursor_column: u16,
    pub cursor: CursorShape,
    /// Set by the shell or the running program.
    pub title: String,
    /// Lines scrolled back into history (0 at the bottom).
    pub scrolled_back: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKey {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Function(u8),
    /// A printable character, or a letter combined with Ctrl/Alt.
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers(pub u8);

impl KeyModifiers {
    pub const SHIFT: u8 = 1;
    pub const ALT: u8 = 1 << 1;
    pub const CONTROL: u8 = 1 << 2;

    #[must_use]
    pub fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

/// A cell position on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPoint {
    pub row: u16,
    pub column: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TerminalError {
    #[error("the shell couldn't be started: {0}")]
    Spawn(String),
}

/// One running shell.
pub trait TerminalSession: Send + Sync {
    fn key(&self, key: TerminalKey, modifiers: KeyModifiers);
    /// Text from a paste or an input method; bracketed if the program asked.
    fn paste(&self, text: &str);
    fn resize(&self, size: TerminalSize);
    fn screen(&self) -> TerminalScreen;
    /// Positive: back into history.
    fn scroll(&self, lines: i32);
    fn select(&self, from: CellPoint, to: CellPoint);
    fn clear_selection(&self);
    fn selected_text(&self) -> Option<String>;
    /// `cd` into `folder` if the shell is waiting at its prompt (nothing else
    /// runs in the foreground); `false` otherwise.
    fn follow(&self, folder: &std::path::Path) -> bool;
    /// False once the shell has exited.
    fn is_running(&self) -> bool;
}

/// Called (from any thread) when the screen changed or the shell exited.
pub type ScreenChanged = Arc<dyn Fn() + Send + Sync>;

pub trait TerminalLauncher: Send + Sync {
    /// Starts the user's login shell in `folder`.
    ///
    /// # Errors
    /// The shell couldn't be started.
    fn open(
        &self,
        folder: PathBuf,
        size: TerminalSize,
        changed: ScreenChanged,
    ) -> Result<Arc<dyn TerminalSession>, TerminalError>;
}
