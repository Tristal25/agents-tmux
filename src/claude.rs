//! Claude Code's own records: the registry of running agents, and the transcripts on disk.

use crate::model::{Agent, Chat, Scope, State};
use crate::tmux;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A turn or a command in flight, published by the agent itself. Anything else means it is waiting.
const WORKING: [&str; 2] = ["busy", "shell"];

/// Reading a transcript backwards needs a window; a title sits within the last entries, and one
/// entry can carry a large tool result.
const TAIL_WINDOW: u64 = 512 * 1024;

/// A registry entry, written per running agent as `~/.claude/sessions/<pid>.json`.
#[derive(Debug, Deserialize)]
struct Registry {
    pid: i32,
    #[serde(rename = "sessionId")]
    session_id: String,
    cwd: Option<String>,
    /// `<session>:<window>.<pane>`, absent when the agent runs outside tmux.
    tmux: Option<String>,
    status: Option<String>,
    #[serde(rename = "statusUpdatedAt")]
    status_updated_at: Option<i64>,
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn projects_dir() -> PathBuf {
    home().join(".claude/projects")
}

/// A live agent, as the registry describes it.
struct Live {
    id: String,
    dir: PathBuf,
    pid: i32,
    session: Option<String>,
    working: bool,
    stamp: i64,
}

/// A registry file outlives its process, and a recycled pid would otherwise read as running, so
/// each entry is confirmed against the process table.
fn process_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        || std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

fn live_agents() -> Vec<Live> {
    let dir = home().join(".claude/sessions");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Ok(reg) = serde_json::from_str::<Registry>(&text) else { continue };
        if !process_alive(reg.pid) {
            continue;
        }
        out.push(Live {
            id: reg.session_id,
            dir: PathBuf::from(reg.cwd.unwrap_or_default()),
            pid: reg.pid,
            // The pane reference carries window and pane after a colon; the session is the part
            // anything can be attached to.
            session: reg
                .tmux
                .filter(|t| t != "-")
                .map(|t| t.split(':').next().unwrap_or(&t).to_string()),
            working: reg
                .status
                .as_deref()
                .map(|s| WORKING.contains(&s))
                .unwrap_or(false),
            stamp: reg.status_updated_at.unwrap_or(0) / 1000,
        });
    }
    out
}

fn mtime(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// The newest write across a conversation and the subagents dispatched from it. A chat whose main
/// loop waits while a subagent works writes nothing itself, so the subagent files are what show it
/// is still going.
fn work_epoch(transcript: &Path) -> i64 {
    let mut newest = mtime(transcript);
    let subagents = transcript.with_extension("").join("subagents");
    if let Ok(entries) = fs::read_dir(subagents) {
        for entry in entries.flatten() {
            newest = newest.max(mtime(&entry.path()));
        }
    }
    newest
}

/// The task the agent named itself, taken from the last `ai-title` in the transcript.
///
/// Claude rewrites that name as the work changes, so the newest one describes the chat best. Only
/// the tail of the file is read: these transcripts reach tens of megabytes, and parsing all of it
/// to reach its final lines costs far more than the answer is worth. Without a title, the last
/// thing typed stands in, skipping the injected shapes nobody typed.
pub fn title_of(transcript: &Path) -> String {
    let Ok(mut file) = File::open(transcript) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_WINDOW);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        // A window boundary can land mid character, and lossy bytes still hold the lines.
        let mut raw = Vec::new();
        let _ = File::open(transcript).and_then(|mut f| {
            f.seek(SeekFrom::Start(start))?;
            f.read_to_end(&mut raw)
        });
        buf = String::from_utf8_lossy(&raw).into_owned();
    }

    let mut fallback = String::new();
    for line in buf.lines().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if value.get("type").and_then(|t| t.as_str()) == Some("ai-title") {
            if let Some(title) = value.get("aiTitle").and_then(|t| t.as_str()) {
                if !title.is_empty() {
                    return title.to_string();
                }
            }
        }
        if fallback.is_empty() && value.get("type").and_then(|t| t.as_str()) == Some("user") {
            if let Some(text) = user_text(&value) {
                fallback = text;
            }
        }
    }
    fallback
}

/// What the person typed, flattened to one line. System reminders, command echoes and caveats are
/// injected rather than typed, so they never stand in for a title.
fn user_text(value: &serde_json::Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    let raw = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .first()
            .and_then(|i| i.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => return None,
    };
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() || flat.starts_with('<') || flat.starts_with("Caveat:") {
        return None;
    }
    Some(flat)
}

/// The directory a conversation belongs to, read from the transcript itself: the project directory
/// name turns every slash into a dash, and a real dash in a path makes that ambiguous.
fn dir_of(transcript: &Path) -> PathBuf {
    if let Ok(text) = fs::read_to_string(transcript) {
        for line in text.lines().take(200) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(cwd) = value.get("cwd").and_then(|c| c.as_str()) {
                    return PathBuf::from(cwd);
                }
            }
        }
    }
    PathBuf::new()
}

/// Every Claude chat, live or saved. One row per conversation: several tmux sessions can hold the
/// same one, and a row each would bury every other chat under repeats of one.
pub fn chats(scope: &Scope, sessions: &HashMap<String, tmux::Session>) -> Vec<Chat> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut by_id: HashMap<String, Chat> = HashMap::new();
    for live in live_agents() {
        if let Scope::Dir(want) = scope {
            if &live.dir != want {
                continue;
            }
        }
        let transcript = transcript_path(&live.id, &live.dir);
        let entry = by_id.entry(live.id.clone()).or_insert_with(|| Chat {
            agent: Agent::Claude,
            id: live.id.clone(),
            state: State::Idle,
            session: None,
            attached: 0,
            held: 0,
            pid: Some(live.pid),
            dir: live.dir.clone(),
            title: title_of(&transcript),
            last_used: 0,
        });
        entry.held += 1;
        // Any agent on this conversation reporting work counts as work, so a busy one is never
        // masked by an idle sibling.
        if live.working {
            entry.state = State::Running;
        }
        entry.last_used = entry.last_used.max(live.stamp);

        // The row points at the session that was active most recently, since that is the one worth
        // joining when several hold the conversation.
        let activity = live
            .session
            .as_ref()
            .and_then(|s| sessions.get(s))
            .map(|s| s.activity)
            .unwrap_or(0);
        let better = entry
            .session
            .as_ref()
            .and_then(|s| sessions.get(s))
            .map(|s| s.activity)
            .unwrap_or(-1);
        if live.session.is_some() && activity >= better {
            entry.session = live.session.clone();
            entry.pid = Some(live.pid);
            entry.attached = sessions
                .get(live.session.as_deref().unwrap_or_default())
                .map(|s| s.attached)
                .unwrap_or(0);
        }

        let worked = work_epoch(&transcript);
        entry.last_used = entry.last_used.max(worked);
        if entry.state != State::Running && now - worked <= crate::IDLE_AFTER {
            entry.state = State::Running;
        }
    }

    // An agent with no session behind it cannot be attached to, whatever it is doing.
    for chat in by_id.values_mut() {
        if chat.session.is_none() {
            chat.state = State::NoTmux;
        }
        if chat.last_used == 0 {
            chat.last_used = now;
        }
    }

    let live_ids: Vec<String> = by_id.keys().cloned().collect();
    let mut out: Vec<Chat> = by_id.into_values().collect();

    for transcript in transcripts(scope) {
        let id = transcript
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if live_ids.contains(&id) {
            continue;
        }
        out.push(Chat {
            agent: Agent::Claude,
            id,
            state: State::Exited,
            session: None,
            attached: 0,
            held: 1,
            pid: None,
            dir: dir_of(&transcript),
            title: title_of(&transcript),
            last_used: mtime(&transcript),
        });
    }
    out
}

fn transcript_path(id: &str, dir: &Path) -> PathBuf {
    let key = dir.to_string_lossy().replace('/', "-");
    projects_dir().join(key).join(format!("{id}.jsonl"))
}

/// Every saved transcript in scope. Subagent transcripts live one level deeper and belong to their
/// parent, so only the top level of each project directory counts.
fn transcripts(scope: &Scope) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match scope {
        Scope::Dir(dir) => {
            let key = dir.to_string_lossy().replace('/', "-");
            dirs.push(projects_dir().join(key));
        }
        Scope::Everywhere => {
            if let Ok(entries) = fs::read_dir(projects_dir()) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        dirs.push(entry.path());
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    out.push(path);
                }
            }
        }
    }
    out
}
