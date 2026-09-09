//! The interactive bits of the CLI: is this a terminal, are we root, read a secret with echo off.
//!
//! The install snippet the UI shows deliberately leaves the `ak_` token OUT of the command line —
//! a token pasted as an argument lands in `~/.bash_history` and in `ps` for every user on the
//! box. Instead, `photon-agent install` (and a foreground run with `--server`) prompts for it
//! when nothing supplied one and stdin is a terminal. Scripts still pass `--token` or
//! `PHOTON_AGENT_TOKEN`; a non-interactive run without either fails with a message that says so.

use std::io::{self, BufRead, Write};

/// True when stdin is an interactive terminal (a human is there to answer a prompt).
pub fn stdin_is_tty() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `isatty` reads the descriptor's type and has no other effect.
        unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// True when the effective user is root. `install` needs it for `/usr/local/bin`,
/// `/etc/photon-agent` and `systemctl`; checking first turns three cryptic "permission denied"
/// lines into one sentence that names the fix.
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` cannot fail and touches nothing.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Read one line from stdin with terminal echo switched off (a password prompt). The prompt
/// goes to stderr so stdout stays clean for anything a script may capture. Returns `None` on
/// EOF or an empty answer.
pub fn read_secret(prompt: &str) -> io::Result<Option<String>> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let _guard = EchoOff::new();
    let mut line = String::new();
    let n = io::stdin().lock().read_line(&mut line)?;
    drop(_guard);
    // The user's Enter was swallowed with the echo; give the terminal its newline back.
    eprintln!();
    if n == 0 {
        return Ok(None);
    }
    let t = line.trim().to_string();
    Ok(if t.is_empty() { None } else { Some(t) })
}

/// Switches terminal echo off for its lifetime and restores the previous flags on drop, so a
/// Ctrl-C mid-prompt cannot leave the shell silent.
struct EchoOff {
    #[cfg(unix)]
    prev: Option<libc::termios>,
}

impl EchoOff {
    fn new() -> Self {
        #[cfg(unix)]
        {
            // SAFETY: a zeroed termios is a valid input for tcgetattr to fill; both calls act
            // only on the stdin descriptor and we restore the exact flags we read.
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                    return Self { prev: None };
                }
                let prev = t;
                t.c_lflag &= !libc::ECHO;
                if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t) != 0 {
                    return Self { prev: None };
                }
                Self { prev: Some(prev) }
            }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(prev) = self.prev {
            // SAFETY: restoring flags we previously read from the same descriptor.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &prev);
            }
        }
    }
}

/// Fill in a missing agent token: prompt when a human is there, else explain how a script
/// supplies it. `what` names the command for the error ("install", "run").
pub fn resolve_token(token: Option<String>, what: &str) -> Result<String, String> {
    if let Some(t) = token.filter(|t| !t.trim().is_empty()) {
        return Ok(t.trim().to_string());
    }
    if !stdin_is_tty() {
        return Err(format!(
            "{what} needs the per-host agent token: pass --token ak_… or set PHOTON_AGENT_TOKEN \
             (stdin is not a terminal, so there is nobody to prompt)"
        ));
    }
    let t = read_secret("Agent token (ak_…, from the Photon UI — input is hidden): ")
        .map_err(|e| format!("reading the token: {e}"))?;
    match t {
        Some(t) if t.starts_with("ak_") => Ok(t),
        Some(_) => Err("that does not look like an agent token (they start with ak_)".into()),
        None => Err("no token entered".into()),
    }
}
