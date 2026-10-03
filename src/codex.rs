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

/// Which tmux session holds a Codex conversation, as its server's socket and its name, keyed by
/// rollout id where the session names it and by directory otherwise.
struct Holders {
    by_id: HashMap<String, (PathBuf, String)>,
    by_dir: HashMap<String, (PathBuf, String)>,
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

/// A folder with its symlinks resolved, so a pane's path and a rollout's `cwd` match when
/// one names the folder through a link, such as `/home/<user>` pointing at `/local/home/<user>`.
fn resolved(path: &str) -> String {
    fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

fn holders(panes: &[tmux::Pane]) -> Holders {
    let by_tty: HashMap<&str, &tmux::Pane> = panes.iter().map(|p| (p.tty.as_str(), p)).collect();
    let mut out = Holders {
        by_id: HashMap::new(),
        by_dir: HashMap::new(),
    };
    for pid in codex_pids() {
        let Some(tty) = crate::term::tty_of(pid) else { continue };
        let Some(pane) = by_tty.get(tty.as_str()) else { continue };
        if let Some(id) = id_in(&pane.start_command) {
            out.by_id.insert(id, (pane.server.clone(), pane.session.clone()));
        }
        out.by_dir.insert(resolved(&pane.path), (pane.server.clone(), pane.session.clone()));
    }
    out
}

/// How many folder levels below `CODEX_HOME` to search. Codex files a rollout at
/// `sessions/YYYY/MM/DD/rollout-*.jsonl`, four folders down, and `rollouts` reads a folder's
/// files only while it has a level left, so the search needs one level past that.
const ROLLOUT_DEPTH: usize = 5;

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
            // Older rollouts carry `cwd` at the top of a line; newer ones nest it in the
            // `session_meta` line's `payload`.
            let cwd = value
                .get("cwd")
                .or_else(|| value.get("payload").and_then(|p| p.get("cwd")))
                .and_then(|c| c.as_str());
            if let Some(cwd) = cwd {
                return Some(PathBuf::from(cwd));
            }
        }
    }
    None
}

pub fn chats(
    scope: &Scope,
    sessions: &tmux::Sessions,
    panes: &[tmux::Pane],
    hidden: &std::collections::HashSet<String>,
) -> Vec<Chat> {
    let held = holders(panes);
    let mut files = Vec::new();
    rollouts(&codex_home(), ROLLOUT_DEPTH, &mut files);

    files
        .into_iter()
        .filter_map(|path| {
            let id = path.file_stem()?.to_string_lossy().into_owned();
            if hidden.contains(&id) {
                return None;
            }
            let dir = first_cwd(&path).unwrap_or_else(|| crate::claude::home());
            if let Scope::Dir(want) = scope {
                if &dir != want {
                    return None;
                }
            }
            let session = held
                .by_id
                .get(&id)
                .or_else(|| held.by_dir.get(&resolved(&dir.to_string_lossy())))
                .cloned();
            let attached = session
                .as_ref()
                .and_then(|(server, name)| sessions.get(server, name))
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
                server: session.as_ref().map(|(server, _)| server.clone()),
                session: session.map(|(_, name)| name),
                attached,
                held: 1,
                pid: None,
                dir,
                last_used,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rollout_four_folders_down_is_found() {
        let stamp = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("agent-tmux-rollouts-{}-{stamp}", std::process::id()));
        let day = root.join("sessions").join("2026").join("10").join("03");
        fs::create_dir_all(&day).unwrap();
        let file = day.join("rollout-2026-10-03T00-58-00-id.jsonl");
        fs::write(&file, "{}\n").unwrap();
        let mut found = Vec::new();
        rollouts(&root, ROLLOUT_DEPTH, &mut found);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(found, vec![file]);
    }

    #[test]
    fn a_folder_named_through_a_link_matches_its_target() {
        let stamp = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("agent-tmux-link-{}-{stamp}", std::process::id()));
        let target = root.join("local").join("home");
        fs::create_dir_all(&target).unwrap();
        let link = root.join("home");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (a, b) = (resolved(&link.to_string_lossy()), resolved(&target.to_string_lossy()));
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn the_folder_is_read_at_the_top_or_inside_the_payload() {
        let stamp = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("agent-tmux-cwd-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let top = root.join("top.jsonl");
        fs::write(&top, "{\"cwd\":\"/work/top\"}\n").unwrap();
        let nested = root.join("nested.jsonl");
        fs::write(&nested, "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/work/nested\"}}\n").unwrap();
        let (a, b) = (first_cwd(&top), first_cwd(&nested));
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(a, Some(PathBuf::from("/work/top")));
        assert_eq!(b, Some(PathBuf::from("/work/nested")));
    }
}
