//! Whether the shell is waiting at its prompt: the terminal's foreground
//! process group is led by a shell (and not, say, `vim` or a build). Only
//! then is a `cd` typed for the user when they change folders.

use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};

/// Processes treated as an idle prompt when in the foreground.
const SHELLS: &[&str] = &["zsh", "bash", "fish", "sh", "dash", "ksh", "mksh", "tcsh", "csh", "nu", "elvish", "xonsh"];
#[cfg(target_os = "macos")]
const PROCESS_NAME_BUFFER: usize = 256;

pub(crate) struct Watcher {
    /// Our own handle on the PTY master, valid as long as we live.
    master: OwnedFd,
}

impl Watcher {
    pub(crate) fn new(master: &File) -> io::Result<Self> {
        Ok(Self { master: master.as_fd().try_clone_to_owned()? })
    }

    pub(crate) fn shell_is_waiting(&self) -> bool {
        // SAFETY: `tcgetpgrp` only reads the process group of a descriptor we own.
        let group = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        group > 0 && process_name(group).is_some_and(|name| is_shell(&name))
    }
}

fn is_shell(name: &str) -> bool {
    // Login shells are named `-zsh`.
    SHELLS.contains(&name.trim_start_matches('-'))
}

/// The name of the process leading the group (its id is the group's).
#[cfg(target_os = "linux")]
fn process_name(pid: libc::pid_t) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|name| name.trim().to_owned())
}

#[cfg(target_os = "macos")]
fn process_name(pid: libc::pid_t) -> Option<String> {
    let mut buffer = [0_u8; PROCESS_NAME_BUFFER];
    let capacity = u32::try_from(buffer.len()).ok()?;
    // SAFETY: the buffer is valid for `capacity` bytes; the call writes at most that.
    let length = unsafe { libc::proc_name(pid, buffer.as_mut_ptr().cast(), capacity) };
    let length = usize::try_from(length).ok().filter(|l| *l > 0)?;
    Some(String::from_utf8_lossy(&buffer[..length]).into_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_name(_pid: libc::pid_t) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_are_recognised_with_or_without_the_login_dash() {
        assert!(is_shell("-zsh"));
        assert!(is_shell("bash"));
        assert!(!is_shell("vim"));
        assert!(!is_shell("zshrc-helper"));
    }

    #[test]
    fn this_process_has_a_name() {
        let pid = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
        assert!(process_name(pid).is_some_and(|name| !name.is_empty()));
    }
}
