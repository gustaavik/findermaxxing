//! `TerminalLauncher` (ADR 0028): the user's login shell on a
//! pseudo-terminal, emulated by `alacritty_terminal` with its own PTY and
//! I/O loop (the code Alacritty itself runs). The UI only ever sees
//! [`TerminalScreen`] snapshots and sends keys back.

mod foreground;
mod keys;
mod screen;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use application::ports::{
    CellPoint, KeyModifiers, ScreenChanged, TerminalError, TerminalKey, TerminalLauncher, TerminalScreen,
    TerminalSession, TerminalSize,
};

/// Set inside every Flatpak sandbox.
const FLATPAK_VARIABLE: &str = "FLATPAK_ID";
const FLATPAK_SPAWN: &str = "flatpak-spawn";
/// Starts the account's login shell, as Terminal apps on the host do.
const HOST_LOGIN_SHELL: &str = r#"exec "$(getent passwd "$(id -un)" | cut -d: -f7)" -l"#;

/// Lines kept above the screen.
const SCROLLBACK_LINES: usize = 10_000;
/// Nominal cell size reported to programs that ask for pixels.
const CELL_WIDTH: u16 = 8;
const CELL_HEIGHT: u16 = 16;
/// Identifies the terminal to programs (`TERM`); `xterm-256color` is what
/// everything understands.
const TERM_NAME: &str = "xterm-256color";
const COLOR_TERM: &str = "truecolor";

#[derive(Debug, Default)]
pub struct AlacrittyLauncher;

/// Receives the emulator's events on its I/O thread.
#[derive(Clone)]
struct Listener {
    changed: ScreenChanged,
    title: Arc<Mutex<String>>,
    running: Arc<AtomicBool>,
    /// Set right after the event loop exists; answers queries from programs.
    sender: Arc<OnceLock<EventLoopSender>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => (self.changed)(),
            Event::Title(title) => *self.title.lock().unwrap_or_else(PoisonError::into_inner) = title,
            Event::ResetTitle => self.title.lock().unwrap_or_else(PoisonError::into_inner).clear(),
            Event::PtyWrite(text) => {
                if let Some(sender) = self.sender.get() {
                    let _ = sender.send(Msg::Input(Cow::Owned(text.into_bytes())));
                }
            }
            Event::ChildExit(_) | Event::Exit => {
                self.running.store(false, Ordering::Relaxed);
                (self.changed)();
            }
            _ => {}
        }
    }
}

struct GridSize(TerminalSize);

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.0.rows.max(1))
    }

    fn columns(&self) -> usize {
        usize::from(self.0.columns.max(1))
    }
}

fn window_size(size: TerminalSize) -> WindowSize {
    WindowSize {
        num_lines: size.rows.max(1),
        num_cols: size.columns.max(1),
        cell_width: CELL_WIDTH,
        cell_height: CELL_HEIGHT,
    }
}

/// Inside a Flatpak sandbox the sandbox's own `sh` is useless: run the
/// user's login shell on the host instead (needs `--talk-name=org.freedesktop.Flatpak`).
fn flatpak_host_shell(folder: &Path) -> Option<tty::Shell> {
    std::env::var_os(FLATPAK_VARIABLE)?;
    let args = [
        "--host".to_owned(),
        "--watch-bus".to_owned(),
        format!("--directory={}", folder.display()),
        format!("--env=TERM={TERM_NAME}"),
        format!("--env=COLORTERM={COLOR_TERM}"),
        "sh".to_owned(),
        "-c".to_owned(),
        HOST_LOGIN_SHELL.to_owned(),
    ];
    Some(tty::Shell::new(FLATPAK_SPAWN.to_owned(), args.to_vec()))
}

impl TerminalLauncher for AlacrittyLauncher {
    fn open(
        &self,
        folder: PathBuf,
        size: TerminalSize,
        changed: ScreenChanged,
    ) -> Result<Arc<dyn TerminalSession>, TerminalError> {
        let spawn_failed = |e: &dyn std::fmt::Display| TerminalError::Spawn(e.to_string());
        let options = tty::Options {
            shell: flatpak_host_shell(&folder),
            working_directory: Some(folder),
            drain_on_exit: false,
            env: [("TERM".to_owned(), TERM_NAME.to_owned()), ("COLORTERM".to_owned(), COLOR_TERM.to_owned())]
                .into_iter()
                .collect(),
        };
        let pty = tty::new(&options, window_size(size), 0).map_err(|e| spawn_failed(&e))?;
        let foreground = foreground::Watcher::new(pty.file()).map_err(|e| spawn_failed(&e))?;
        let listener = Listener {
            changed,
            title: Arc::default(),
            running: Arc::new(AtomicBool::new(true)),
            sender: Arc::default(),
        };
        let config = Config { scrolling_history: SCROLLBACK_LINES, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &GridSize(size), listener.clone())));
        let event_loop =
            EventLoop::new(Arc::clone(&term), listener.clone(), pty, false, false).map_err(|e| spawn_failed(&e))?;
        let sender = event_loop.channel();
        let _ = listener.sender.set(event_loop.channel());
        event_loop.spawn();
        Ok(Arc::new(Session { term, sender, listener, foreground }))
    }
}

struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    listener: Listener,
    foreground: foreground::Watcher,
}

impl Session {
    fn write(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.sender.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    fn modes(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// Typing jumps back to the prompt, as in every terminal.
    fn to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    fn grid_point(&self, cell: CellPoint) -> Point {
        let term = self.term.lock();
        let offset = i32::try_from(term.grid().display_offset()).unwrap_or(0);
        let column = usize::from(cell.column).min(term.columns().saturating_sub(1));
        Point::new(Line(i32::from(cell.row) - offset), Column(column))
    }
}

impl TerminalSession for Session {
    fn key(&self, key: TerminalKey, modifiers: KeyModifiers) {
        self.to_bottom();
        let bytes = keys::encode(key, modifiers, screen::key_modes(self.modes()));
        self.write(bytes);
    }

    fn paste(&self, text: &str) {
        self.to_bottom();
        let bytes = keys::paste(text, screen::key_modes(self.modes()));
        self.write(bytes);
    }

    fn resize(&self, size: TerminalSize) {
        self.term.lock().resize(GridSize(size));
        let _ = self.sender.send(Msg::Resize(window_size(size)));
    }

    fn screen(&self) -> TerminalScreen {
        let title = self.listener.title.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let term = self.term.lock();
        screen::snapshot(term.renderable_content(), &*term, title)
    }

    fn scroll(&self, lines: i32) {
        let modes = self.modes();
        if modes.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // Full-screen programs (less, vim) scroll with arrow keys.
            let key = if lines > 0 { TerminalKey::Up } else { TerminalKey::Down };
            let bytes = keys::encode(key, KeyModifiers::default(), screen::key_modes(modes));
            self.write(bytes.repeat(usize::try_from(lines.unsigned_abs()).unwrap_or(0)));
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            (self.listener.changed)();
        }
    }

    fn select(&self, from: CellPoint, to: CellPoint) {
        let (start, end) = (self.grid_point(from), self.grid_point(to));
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        self.term.lock().selection = Some(selection);
        (self.listener.changed)();
    }

    fn clear_selection(&self) {
        self.term.lock().selection = None;
        (self.listener.changed)();
    }

    fn selected_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|text| !text.is_empty())
    }

    fn follow(&self, folder: &Path) -> bool {
        if !self.foreground.shell_is_waiting() {
            return false;
        }
        // Ctrl-U clears whatever is half-typed; the leading space keeps the
        // command out of most shells' history.
        let quoted = folder.to_string_lossy().replace('\'', "'\\''");
        self.write(format!("\x15 cd -- '{quoted}'\r").into_bytes());
        true
    }

    fn is_running(&self) -> bool {
        self.listener.running.load(Ordering::Relaxed)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}
