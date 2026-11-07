//! Terminal colours for the light and dark themes: the 16 ANSI colours
//! (tuned per theme for contrast), xterm's 6×6×6 cube and grey ramp, and
//! the default text, background and selection colours. ARGB, opaque.

use application::ports::TerminalColor;

const OPAQUE: u32 = 0xff00_0000;
/// Levels of xterm's colour cube (indices 16–231).
const CUBE_LEVELS: [u32; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
const CUBE_START: u8 = 16;
const GREY_START: u8 = 232;
const GREY_BASE: u32 = 8;
const GREY_STEP: u32 = 10;

const DARK_ANSI: [u32; 16] = [
    0x1d1f21, 0xe0605f, 0x8cc265, 0xd9b15b, 0x6aa7e8, 0xc678dd, 0x56b6c2, 0xc5c8c6, //
    0x666a70, 0xff7b72, 0xa5d6a7, 0xf2cc60, 0x82b6ff, 0xd8a0f0, 0x76d6e0, 0xf5f5f5,
];
const LIGHT_ANSI: [u32; 16] = [
    0x1f2328, 0xc0392b, 0x2e7d32, 0x8a6d00, 0x1f5fbf, 0x8e44ad, 0x00838f, 0x6e7781, //
    0x57606a, 0xd73a49, 0x22863a, 0xa66a00, 0x0366d6, 0x9b4dca, 0x1b7c83, 0x24292f,
];

#[derive(Debug, Clone, Copy)]
pub(crate) struct Palette {
    ansi: &'static [u32; 16],
    foreground: u32,
    background: u32,
    selection: u32,
}

impl Palette {
    pub(crate) fn for_theme(dark: bool) -> Self {
        if dark {
            Self { ansi: &DARK_ANSI, foreground: 0xd8dade, background: 0x1e1f22, selection: 0x2f4f7a }
        } else {
            Self { ansi: &LIGHT_ANSI, foreground: 0x1f2328, background: 0xfbfbfb, selection: 0xb6d4fe }
        }
    }

    pub(crate) fn background(&self) -> u32 {
        OPAQUE | self.background
    }

    pub(crate) fn selection(&self) -> u32 {
        OPAQUE | self.selection
    }

    pub(crate) fn resolve(&self, color: TerminalColor) -> u32 {
        OPAQUE
            | match color {
                TerminalColor::Foreground => self.foreground,
                TerminalColor::Background => self.background,
                TerminalColor::Rgb(r, g, b) => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
                TerminalColor::Indexed(index) if index < CUBE_START => self.ansi[usize::from(index)],
                TerminalColor::Indexed(index) if index < GREY_START => {
                    let cube = u32::from(index - CUBE_START);
                    let level = |i: u32| CUBE_LEVELS[usize::try_from(i).unwrap_or(0)];
                    (level(cube / 36) << 16) | (level(cube / 6 % 6) << 8) | level(cube % 6)
                }
                TerminalColor::Indexed(index) => {
                    let grey = GREY_BASE + GREY_STEP * u32::from(index - GREY_START);
                    (grey << 16) | (grey << 8) | grey
                }
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cube_and_grey_ramp_match_xterm() {
        let palette = Palette::for_theme(false);
        assert_eq!(palette.resolve(TerminalColor::Indexed(196)), 0xffff_0000, "pure red");
        assert_eq!(palette.resolve(TerminalColor::Indexed(16)), 0xff00_0000, "cube black");
        assert_eq!(palette.resolve(TerminalColor::Indexed(232)), 0xff08_0808);
        assert_eq!(palette.resolve(TerminalColor::Indexed(255)), 0xffee_eeee);
        assert_eq!(palette.resolve(TerminalColor::Rgb(1, 2, 3)), 0xff01_0203);
        assert_ne!(Palette::for_theme(true).background(), palette.background());
    }
}
