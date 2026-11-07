//! Keys to the bytes an xterm-compatible program expects.

use application::ports::{KeyModifiers, TerminalKey};

const ESC: &str = "\x1b";
const DELETE: u8 = 0x7f;
const BACKSPACE_WITH_CONTROL: u8 = 0x08;
const CONTROL_MASK: u8 = 0x1f;
const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

/// The modes of the running program that change what keys send.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct KeyModes {
    /// `DECCKM`: arrows send `ESC O x` instead of `ESC [ x`.
    pub application_cursor: bool,
    pub bracketed_paste: bool,
}

/// xterm's modifier parameter: 1 + shift + 2·alt + 4·control.
fn modifier_parameter(modifiers: KeyModifiers) -> u8 {
    1 + u8::from(modifiers.has(KeyModifiers::SHIFT))
        + 2 * u8::from(modifiers.has(KeyModifiers::ALT))
        + 4 * u8::from(modifiers.has(KeyModifiers::CONTROL))
}

fn cursor_key(letter: char, modifiers: KeyModifiers, modes: KeyModes) -> Vec<u8> {
    match modifier_parameter(modifiers) {
        1 if modes.application_cursor => format!("{ESC}O{letter}"),
        1 => format!("{ESC}[{letter}"),
        parameter => format!("{ESC}[1;{parameter}{letter}"),
    }
    .into_bytes()
}

fn tilde_key(number: u8, modifiers: KeyModifiers) -> Vec<u8> {
    match modifier_parameter(modifiers) {
        1 => format!("{ESC}[{number}~"),
        parameter => format!("{ESC}[{number};{parameter}~"),
    }
    .into_bytes()
}

fn function_key(number: u8, modifiers: KeyModifiers) -> Vec<u8> {
    match number {
        1..=4 => {
            let letter = char::from(b'P' + number - 1);
            match modifier_parameter(modifiers) {
                1 => format!("{ESC}O{letter}").into_bytes(),
                parameter => format!("{ESC}[1;{parameter}{letter}").into_bytes(),
            }
        }
        // F5 is 15; F6–F10 are 17–21; F11, F12 are 23, 24 (xterm skips 16 and 22).
        5 => tilde_key(15, modifiers),
        6..=10 => tilde_key(number + 11, modifiers),
        11 | 12 => tilde_key(number + 12, modifiers),
        _ => Vec::new(),
    }
}

fn character(c: char, modifiers: KeyModifiers) -> Vec<u8> {
    let mut bytes = Vec::new();
    if modifiers.has(KeyModifiers::ALT) {
        bytes.extend_from_slice(ESC.as_bytes());
    }
    let control = modifiers.has(KeyModifiers::CONTROL);
    match c {
        // Ctrl-A … Ctrl-Z, Ctrl-@ [ \ ] ^ _ and Ctrl-Space.
        'a'..='z' | 'A'..='Z' | '@' | '[' | '\\' | ']' | '^' | '_' if control => {
            bytes.push(c.to_ascii_uppercase() as u8 & CONTROL_MASK);
        }
        ' ' if control => bytes.push(0),
        '?' if control => bytes.push(DELETE),
        c => bytes.extend_from_slice(c.to_string().as_bytes()),
    }
    bytes
}

/// The bytes for a key press.
pub(crate) fn encode(key: TerminalKey, modifiers: KeyModifiers, modes: KeyModes) -> Vec<u8> {
    let alt_prefixed = |bytes: &[u8]| {
        let mut out = if modifiers.has(KeyModifiers::ALT) { ESC.as_bytes().to_vec() } else { Vec::new() };
        out.extend_from_slice(bytes);
        out
    };
    match key {
        TerminalKey::Enter => alt_prefixed(b"\r"),
        TerminalKey::Backspace if modifiers.has(KeyModifiers::CONTROL) => alt_prefixed(&[BACKSPACE_WITH_CONTROL]),
        TerminalKey::Backspace => alt_prefixed(&[DELETE]),
        TerminalKey::Tab if modifiers.has(KeyModifiers::SHIFT) => format!("{ESC}[Z").into_bytes(),
        TerminalKey::Tab => alt_prefixed(b"\t"),
        TerminalKey::Escape => alt_prefixed(ESC.as_bytes()),
        TerminalKey::Up => cursor_key('A', modifiers, modes),
        TerminalKey::Down => cursor_key('B', modifiers, modes),
        TerminalKey::Right => cursor_key('C', modifiers, modes),
        TerminalKey::Left => cursor_key('D', modifiers, modes),
        TerminalKey::Home => cursor_key('H', modifiers, modes),
        TerminalKey::End => cursor_key('F', modifiers, modes),
        TerminalKey::Insert => tilde_key(2, modifiers),
        TerminalKey::Delete => tilde_key(3, modifiers),
        TerminalKey::PageUp => tilde_key(5, modifiers),
        TerminalKey::PageDown => tilde_key(6, modifiers),
        TerminalKey::Function(number) => function_key(number, modifiers),
        TerminalKey::Char(c) => character(c, modifiers),
    }
}

/// Pasted text: bracketed when the program asked (so it isn't run line by
/// line), with escape characters removed so a paste can't end the bracket.
pub(crate) fn paste(text: &str, modes: KeyModes) -> Vec<u8> {
    if modes.bracketed_paste {
        let clean: String = text.chars().filter(|c| *c != '\x1b').collect();
        format!("{BRACKETED_PASTE_START}{clean}{BRACKETED_PASTE_END}").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: KeyModifiers = KeyModifiers(0);
    const NORMAL: KeyModes = KeyModes { application_cursor: false, bracketed_paste: false };

    fn key(key: TerminalKey, modifiers: u8) -> String {
        String::from_utf8(encode(key, KeyModifiers(modifiers), NORMAL)).unwrap_or_default()
    }

    #[test]
    fn control_letters_and_alt_prefixes() {
        assert_eq!(key(TerminalKey::Char('c'), KeyModifiers::CONTROL), "\x03");
        assert_eq!(key(TerminalKey::Char('D'), KeyModifiers::CONTROL), "\x04");
        assert_eq!(key(TerminalKey::Char('b'), KeyModifiers::ALT), "\x1bb");
        assert_eq!(key(TerminalKey::Char('é'), 0), "é");
        assert_eq!(key(TerminalKey::Backspace, 0), "\x7f");
        assert_eq!(key(TerminalKey::Enter, 0), "\r");
        assert_eq!(key(TerminalKey::Tab, KeyModifiers::SHIFT), "\x1b[Z");
    }

    #[test]
    fn cursor_keys_follow_the_mode_and_modifiers() {
        assert_eq!(key(TerminalKey::Up, 0), "\x1b[A");
        let app = KeyModes { application_cursor: true, ..NORMAL };
        assert_eq!(encode(TerminalKey::Up, NONE, app), b"\x1bOA");
        assert_eq!(key(TerminalKey::Right, KeyModifiers::CONTROL), "\x1b[1;5C");
        assert_eq!(key(TerminalKey::Left, KeyModifiers::ALT | KeyModifiers::SHIFT), "\x1b[1;4D");
        assert_eq!(key(TerminalKey::PageDown, 0), "\x1b[6~");
        assert_eq!(key(TerminalKey::Delete, KeyModifiers::SHIFT), "\x1b[3;2~");
    }

    #[test]
    fn function_keys_skip_xterms_gaps() {
        assert_eq!(key(TerminalKey::Function(1), 0), "\x1bOP");
        assert_eq!(key(TerminalKey::Function(5), 0), "\x1b[15~");
        assert_eq!(key(TerminalKey::Function(6), 0), "\x1b[17~");
        assert_eq!(key(TerminalKey::Function(11), 0), "\x1b[23~");
    }

    #[test]
    fn pastes_are_bracketed_when_asked_and_cannot_escape() {
        assert_eq!(paste("a\nb", NORMAL), b"a\rb");
        let bracketed = KeyModes { bracketed_paste: true, ..NORMAL };
        assert_eq!(paste("rm\x1b[201~ -rf", bracketed), b"\x1b[200~rm[201~ -rf\x1b[201~");
    }
}
