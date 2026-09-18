//! The one place that talks to tmux, so every other module works on parsed values.

use std::collections::HashMap;
use std::io;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub name: String,
    /// Seconds since the epoch of the session's last activity.
    pub activity: i64,
    pub attached: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Pane {
    pub session: String,
    pub tty: String,
    pub path: String,
    /// The command the session was created with, which names the conversation for sessions this
    /// tool created.
    pub start_command: String,
}

/// A tab separator keeps the fields unambiguous, since a session name may contain almost anything
/// and a path certainly can.
const SEP: char = '\t';

fn run(args: &[&str]) -> io::Result<String> {
    let out = Command::new("tmux").args(args).output()?;
    // No server running is the ordinary case on a fresh machine, so it reads as an empty list.
    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn sessions() -> HashMap<String, Session> {
    let fmt = format!(
        "#{{session_name}}{SEP}#{{session_activity}}{SEP}#{{session_attached}}",
        SEP = SEP
    );
    let mut map = HashMap::new();
    let text = run(&["list-sessions", "-F", &fmt]).unwrap_or_default();
    for line in text.lines() {
        let mut it = line.split(SEP);
        let (Some(name), Some(activity), Some(attached)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        map.insert(
            name.to_string(),
            Session {
                name: name.to_string(),
                activity: activity.parse().unwrap_or(0),
                attached: attached.parse().unwrap_or(0),
            },
        );
    }
    map
}

pub fn panes() -> Vec<Pane> {
    let fmt = format!(
        "#{{pane_tty}}{SEP}#{{session_name}}{SEP}#{{pane_current_path}}{SEP}#{{pane_start_command}}",
        SEP = SEP
    );
    let text = run(&["list-panes", "-a", "-F", &fmt]).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut it = line.splitn(4, SEP);
            Some(Pane {
                tty: it.next()?.to_string(),
                session: it.next()?.to_string(),
                path: it.next()?.to_string(),
                start_command: it.next().unwrap_or("").to_string(),
            })
        })
        .filter(|p| !p.tty.is_empty())
        .collect()
}

pub fn has_session(name: &str) -> bool {
    // The `=` prefix asks tmux for an exact match, so a name that merely prefixes another one does
    // not report the wrong session.
    // The answer is the exit status, and a missing session is the expected half of it. `output`
    // captures what tmux says about that, where `status` would let its complaint through to the screen
    // the picker is drawing on.
    Command::new("tmux")
        .args(["has-session", "-t", &format!("={name}")])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub fn pane_path(session: &str) -> Option<String> {
    let out = run(&[
        "display",
        "-pt",
        &format!("={session}"),
        "#{pane_current_path}",
    ])
    .ok()?;
    let path = out.trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// A free session name derived from a directory, since tmux needs a name and it is bookkeeping
/// rather than a choice. Anything outside tmux's alphabet becomes a dash.
pub fn free_name(agent: &str, dir: &std::path::Path) -> String {
    let base: String = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = base.trim_matches('-').to_string();
    let stem = if base.is_empty() {
        agent.to_string()
    } else {
        format!("{agent}-{base}")
    };
    if !has_session(&stem) {
        return stem;
    }
    (2..)
        .map(|n| format!("{stem}-{n}"))
        .find(|candidate| !has_session(candidate))
        .unwrap_or(stem)
}
