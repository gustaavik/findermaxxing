//! A real login shell on a real PTY: commands run, colours arrive, and the
//! shell follows folder changes only while it waits at the prompt.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use adapter_terminal::AlacrittyLauncher;
use application::ports::{
    KeyModifiers, TerminalColor, TerminalKey, TerminalLauncher, TerminalScreen, TerminalSession, TerminalSize,
};

const PATIENCE: Duration = Duration::from_secs(20);

fn text(screen: &TerminalScreen) -> String {
    screen
        .rows
        .iter()
        .map(|row| row.iter().map(|run| run.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for(
    session: &dyn TerminalSession,
    changes: &mpsc::Receiver<()>,
    check: impl Fn(&TerminalScreen) -> bool,
) -> TerminalScreen {
    let started = Instant::now();
    loop {
        let screen = session.screen();
        if check(&screen) {
            return screen;
        }
        assert!(started.elapsed() < PATIENCE, "gave up waiting; screen:\n{}", text(&screen));
        let _ = changes.recv_timeout(Duration::from_millis(200));
    }
}

fn type_line(session: &dyn TerminalSession, line: &str) {
    session.paste(line);
    session.key(TerminalKey::Enter, KeyModifiers::default());
}

#[test]
fn a_shell_runs_commands_shows_colours_and_follows_folders() {
    let start = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let (notify, changes) = mpsc::channel();
    let changed = Arc::new(move || {
        let _ = notify.send(());
    });
    let size = TerminalSize { columns: 100, rows: 30 };
    let session = AlacrittyLauncher.open(start.path().to_path_buf(), size, changed).unwrap();
    let session = &*session;
    // Wait until the shell owns the terminal (its prompt is up).
    let started = Instant::now();
    while !session.follow(start.path()) {
        assert!(started.elapsed() < PATIENCE, "the shell never reached its prompt:\n{}", text(&session.screen()));
        std::thread::sleep(Duration::from_millis(100));
    }

    type_line(session, "echo $((6*7))xyz");
    wait_for(session, &changes, |s| text(s).contains("42xyz"));

    type_line(session, "printf '\\033[31mRED\\033[0m\\n'");
    let screen = wait_for(session, &changes, |s| {
        s.rows.iter().flatten().any(|r| r.text.contains("RED") && r.foreground == TerminalColor::Indexed(1))
    });
    assert_eq!(screen.size, size);

    assert!(session.follow(other.path()), "the shell is at its prompt");
    type_line(session, "pwd");
    let canonical = other.path().canonicalize().unwrap();
    let name = canonical.file_name().unwrap().to_string_lossy().into_owned();
    wait_for(session, &changes, |s| text(s).lines().any(|l| l.trim_end().ends_with(&name) && !l.contains("cd --")));

    type_line(session, "sleep 3");
    std::thread::sleep(Duration::from_millis(800));
    assert!(!session.follow(start.path()), "a running command must not get a cd typed into it");

    session.key(TerminalKey::Char('c'), KeyModifiers(KeyModifiers::CONTROL));
    type_line(session, "exit");
    let started = Instant::now();
    while session.is_running() {
        assert!(started.elapsed() < PATIENCE, "the shell didn't exit");
        let _ = changes.recv_timeout(Duration::from_millis(200));
    }
}
