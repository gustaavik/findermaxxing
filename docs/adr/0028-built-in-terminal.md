# ADR 0028 — The built-in terminal

**Status:** accepted (M7a)

## Context
The spec asks for a built-in terminal pane. There is no LGPL Qt terminal widget: QTermWidget, Konsole's part and
Yakuake are GPL. It also has to run full-screen programs (vim, htop, less), not just show command output.

## Decision
- **Emulation.** `alacritty_terminal` (Apache-2.0), with its own PTY and I/O loop: the code Alacritty itself runs,
  covering the login shell, `SIGWINCH`, xterm's escape sequences, scrollback and selection.
  - `TERM=xterm-256color`, `COLORTERM=truecolor`.
  - Programs' queries (cursor position, device attributes) are answered through the event listener.
- **Port** (`application::ports::terminal`):
  - `TerminalLauncher::open(folder, size, changed)` returns a `TerminalSession`.
  - The session takes keys, pastes, resizes, scrolling and selection, and returns `TerminalScreen` snapshots:
    rows of styled runs (colours as `Foreground` / `Background` / indexed / RGB), the cursor and the title.
  - The core never sees escape sequences. `OpenTerminal` starts local folders in place, and folders on servers,
    buckets or drives in the home folder.
- **Keys** are encoded in the adapter (`keys.rs`), because what they send depends on the program's modes:
  - **Cursor keys:** application cursor mode (DECCKM) changes what they send.
  - **Modifiers:** xterm parameters for Shift, Alt and Ctrl combinations.
  - **Function keys:** xterm's numbering, including its gaps.
  - **Pastes:** bracketed when the program asked, with ESC removed so a paste can't end the bracket.
  - **Platform conventions:**
    - macOS: ⌃ is Control, ⌥ composes characters as everywhere else on the Mac, and ⌘C/⌘V copy and paste.
    - Linux: Alt sends ESC, and Ctrl+Shift+C/V copy and paste.
- **Drawing** is a C++ `QQuickPaintedItem` (`cpp/terminal_view.*`). cxx-qt can't subclass it, so this is a
  flagged cxx-qt limitation.
  - It paints runs that Rust pushes in, with colours already resolved for the light or dark theme
    (`terminal_palette.rs`: 16 tuned ANSI colours, xterm's cube and grey ramp).
  - Characters are drawn cell by cell, so wide glyphs and fallback fonts can't shift a row.
  - It emits keys, input-method text, wheel lines and selection back as signals.
  - Redraws are coalesced: one queued redraw per burst of output.
- **Following the folder.** When the user changes folders, `cd -- '<path>'` is typed for them, with Ctrl-U first
  and a leading space to keep it out of history. This happens only if the terminal's foreground process group is
  led by a shell (`tcgetpgrp` on our own copy of the PTY master; `proc_name` on macOS, `/proc/<pid>/comm` on
  Linux). A running vim or build never gets keystrokes injected.
- **Lifetime.** One shell per window, shown and hidden with Ctrl+` or the `>_` button. Closing the pane ends the
  shell, which gets `SIGHUP` as in any terminal.

## Consequences
- The PTY loop and the emulator come from Alacritty, so terminal behaviour inherits its correctness and
  maintenance; the adapter is under 600 lines, tests included.
- **Not supported yet:** mouse reporting to programs (clicking in vim, htop), double-click word selection,
  ligatures, and images (sixel/kitty).
- `adapter-terminal/tests/shell.rs` runs a real login shell on each CI runner: commands, colours,
  following and exit.
