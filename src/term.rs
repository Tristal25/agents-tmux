//! What the terminal owning this session needs to be told, and how to reach it.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Command;

/// The pane this process draws on.
///
/// A process spawned by an agent, such as a hook, has no controlling terminal of its own, so its
/// own entry reports nothing. The agent that spawned it owns the pane, so the terminal is the first
/// one found walking up the ancestry.
pub fn owning_tty() -> Option<String> {
    let mut pid = std::process::id() as i32;
    for _ in 0..6 {
        pid = parent_of(pid)?;
        if let Some(tty) = tty_of(pid) {
            return Some(tty);
        }
    }
    None
}

fn parent_of(pid: i32) -> Option<i32> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn tty_of(pid: i32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let tty = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!tty.is_empty() && tty != "?").then(|| format!("/dev/{tty}"))
}

/// Tell the owning terminal which directory this chat works in.
///
/// A terminal learns that from OSC 7, and tmux consumes the sequence to fill its own per-pane path
/// rather than passing it outward, so a terminal across an ssh connection otherwise keeps showing
/// wherever the login shell started. Sending it before tmux takes the screen is what makes that
/// terminal name the right folder.
pub fn announce_dir(dir: &Path) {
    let host = hostname();
    print!("\x1b]7;file://{host}{}\x1b\\", dir.display());
    let _ = std::io::stdout().flush();
}

fn hostname() -> String {
    Command::new("hostname")
        .arg("-f")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_default()
}

/// Raise a desktop notification on the terminal owning this session, from wherever the agent runs.
///
/// The terminal reads notifications out of the byte stream, so a sequence written to the pane's tty
/// crosses an ssh connection and reaches the app drawing it. tmux would consume it, so inside tmux
/// it travels wrapped in a passthrough sequence, which needs `allow-passthrough on`. Each escape
/// inside that wrapper is doubled, per tmux's rule, and the notification ends on a BEL so its
/// introducer is the only one to double.
pub fn notify(body: &str) -> std::io::Result<()> {
    let Some(tty) = owning_tty() else {
        return Ok(());
    };
    let payload = if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;\x1b\x1b]9;{body}\x07\x1b\\")
    } else {
        format!("\x1b]9;{body}\x07")
    };
    let mut file = OpenOptions::new().write(true).open(tty)?;
    file.write_all(payload.as_bytes())
}
