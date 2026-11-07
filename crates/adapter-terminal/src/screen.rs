//! The emulator's visible grid as runs of equally styled cells.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{RenderableContent, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape as AnsiCursor, NamedColor};
use application::ports::{CursorShape, StyledRun, TerminalColor, TerminalScreen, TerminalSize, TextStyle};

/// Palette entries 0–15 are the named ANSI colours.
const NAMED_PALETTE_SIZE: usize = 16;

fn color(color: Color, default: TerminalColor) -> TerminalColor {
    match color {
        Color::Spec(rgb) => TerminalColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => TerminalColor::Indexed(index),
        Color::Named(named) => match named as usize {
            index if index < NAMED_PALETTE_SIZE => TerminalColor::Indexed(u8::try_from(index).unwrap_or(0)),
            _ if matches!(named, NamedColor::Background) => TerminalColor::Background,
            _ if matches!(named, NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground) => {
                TerminalColor::Foreground
            }
            _ => default,
        },
    }
}

fn style(flags: Flags, selected: bool) -> TextStyle {
    let table = [
        (Flags::BOLD, TextStyle::BOLD),
        (Flags::ITALIC, TextStyle::ITALIC),
        (Flags::ALL_UNDERLINES, TextStyle::UNDERLINE),
        (Flags::DIM, TextStyle::DIM),
        (Flags::STRIKEOUT, TextStyle::STRIKEOUT),
    ];
    let mut bits = table.iter().filter(|(flag, _)| flags.intersects(*flag)).fold(0, |bits, (_, bit)| bits | bit);
    if selected {
        bits |= TextStyle::SELECTED;
    }
    TextStyle(bits)
}

fn cursor_shape(shape: AnsiCursor) -> CursorShape {
    match shape {
        AnsiCursor::Block | AnsiCursor::HollowBlock => CursorShape::Block,
        AnsiCursor::Underline => CursorShape::Underline,
        AnsiCursor::Beam => CursorShape::Bar,
        AnsiCursor::Hidden => CursorShape::Hidden,
    }
}

/// Builds the snapshot; `grid` gives the visible size.
pub(crate) fn snapshot(content: RenderableContent<'_>, grid: &impl Dimensions, title: String) -> TerminalScreen {
    let rows_count = grid.screen_lines();
    let columns = u16::try_from(grid.columns()).unwrap_or(u16::MAX);
    let offset = i32::try_from(content.display_offset).unwrap_or(i32::MAX);
    let mut rows: Vec<Vec<StyledRun>> = vec![Vec::new(); rows_count];
    for indexed in content.display_iter {
        let cell = indexed.cell;
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let Some(row) = usize::try_from(indexed.point.line.0 + offset).ok().and_then(|r| rows.get_mut(r)) else {
            continue;
        };
        let selected = content.selection.is_some_and(|s| s.contains(indexed.point));
        let (mut foreground, mut background) =
            (color(cell.fg, TerminalColor::Foreground), color(cell.bg, TerminalColor::Background));
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut foreground, &mut background);
        }
        let style = style(cell.flags, selected);
        let shown = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
        let width: u16 = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        let column = u16::try_from(indexed.point.column.0).unwrap_or(u16::MAX);
        match row.last_mut() {
            Some(run)
                if run.foreground == foreground
                    && run.background == background
                    && run.style == style
                    && run.column + run.cells == column =>
            {
                run.text.push(shown);
                run.cells += width;
            }
            _ => row.push(StyledRun { column, text: shown.to_string(), cells: width, foreground, background, style }),
        }
        if let Some(zero_width) = cell.zerowidth()
            && let Some(run) = row.last_mut()
        {
            run.text.extend(zero_width);
        }
    }
    let cursor_row = content.cursor.point.line.0 + offset;
    let visible = (0..i32::try_from(rows_count).unwrap_or(i32::MAX)).contains(&cursor_row);
    TerminalScreen {
        size: TerminalSize { columns, rows: u16::try_from(rows_count).unwrap_or(u16::MAX) },
        rows,
        cursor_row: u16::try_from(cursor_row.max(0)).unwrap_or(0),
        cursor_column: u16::try_from(content.cursor.point.column.0).unwrap_or(0),
        cursor: if visible { cursor_shape(content.cursor.shape) } else { CursorShape::Hidden },
        title,
        scrolled_back: content.display_offset,
    }
}

/// Modes that change what keys send.
pub(crate) fn key_modes(mode: TermMode) -> crate::keys::KeyModes {
    crate::keys::KeyModes {
        application_cursor: mode.contains(TermMode::APP_CURSOR),
        bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
    }
}
