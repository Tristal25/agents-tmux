//! Codex conversations. Codex registers nothing about itself, so a live one is found through the
//! terminal it sits in: the process reports a tty, and tmux reports which pane owns that tty.

use crate::model::{Agent, Chat, Scope, State};
use crate::tmux;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::claude::home().join(".codex"))
}

/// Which tmux session holds a Codex conversation, keyed by rollout id where the session names it
/// and by directory otherwise.
struct Holders {
    by_id: HashMap<String, String>,
    by_dir: HashMap<String, String>,
}

fn tty_of(pid: i32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let tty = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!tty.is_empty() && tty != "?").then(|| format!("/dev/{tty}"))
}

fn codex_pids() -> Vec<i32> {
    let Ok(out) = Command::new("pgrep").args(["-x", "codex"]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// A session this tool created runs `codex resume '<id>'`, which names the conversation exactly.
fn id_in(start_command: &str) -> Option<String> {
    let after = start_command.split("codex resume '").nth(1)?;
    let id = after.split('\'').next()?;
    (!id.is_empty()).then(|| id.to_string())
}

fn holders(panes: &[tmux::Pane]) -> Holders {
    let by_tty: HashMap<&str, &tmux::Pane> = panes.iter().map(|p| (p.tty.as_str(), p)).collect();
    let mut out = Holders {
        by_id: HashMap::new(),
        by_dir: HashMap::new(),
    };
    for pid in codex_pids() {
        let Some(tty) = tty_of(pid) else { continue };
        let Some(pane) = by_tty.get(tty.as_str()) else { continue };
        if let Some(id) = id_in(&pane.start_command) {
            out.by_id.insert(id, pane.session.clone());
        }
        out.by_dir.insert(pane.path.clone(), pane.session.clone());
    }
    out
}

/// Every rollout Codex has written. Its own layout has moved between versions, so both the flat
/// `rollout-*.jsonl` name and the `sessions/` tree are searched.
fn rollouts(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rollouts(&path, depth - 1, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let in_sessions = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n == "sessions")
                .unwrap_or(false);
            if in_sessions || name.map(|n| n.starts_with("rollout-")).unwrap_or(false) {
                out.push(path);
            }
        }
    }
}

fn first_cwd(path: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines().take(200) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(cwd) = value.get("cwd").and_then(|c| c.as_str()) {
                return Some(PathBuf::from(cwd));
            }
        }
    }
    None
}

pub fn chats(scope: &Scope, sessions: &HashMap<String, tmux::Session>, panes: &[tmux::Pane]) -> Vec<Chat> {
    let held = holders(panes);
    let mut files = Vec::new();
    rollouts(&codex_home(), 4, &mut files);

    files
        .into_iter()
        .filter_map(|path| {
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let dir = first_cwd(&path).unwrap_or_else(|| crate::claude::home());
            if let Scope::Dir(want) = scope {
                if &dir != want {
                    return None;
                }
            }
            let session = held
                .by_id
                .get(&id)
                .or_else(|| held.by_dir.get(&dir.to_string_lossy().into_owned()))
                .cloned();
            let attached = session
                .as_ref()
                .and_then(|s| sessions.get(s))
                .map(|s| s.attached)
                .unwrap_or(0);
            let last_used = fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(|t| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
                .unwrap_or(0);
            Some(Chat {
                agent: Agent::Codex,
                // Codex names nothing, so the directory it works in is the most useful label.
                title: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| id.clone()),
                state: if session.is_some() { State::Open } else { State::Exited },
                id,
                session,
                attached,
                held: 1,
                pid: None,
                dir,
                last_used,
            })
        })
        .collect()
}
