//! Claude Code's own records: the registry of running agents, and the transcripts on disk.

use crate::model::{Agent, Chat, Scope, State};
use crate::tmux;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A turn or a command in flight, published by the agent itself. Anything else means it is waiting.
const WORKING: [&str; 2] = ["busy", "shell"];

/// Reading a transcript backwards needs a window; a title sits within the last entries, and one
/// entry can carry a large tool result.
const TAIL_WINDOW: u64 = 512 * 1024;

/// How many chats the list carries at most. Reading one costs a head and a tail of its transcript, so
/// the newest are read first and the rest are never opened: the cost follows this number rather than
/// however many conversations have piled up on the machine. A conversation old enough to fall off the
/// end is one the agent's own `--resume` picker still reaches.
pub fn max_chats() -> usize {
    std::env::var("AGENT_TMUX_MAX_CHATS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(50)
}

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
    /// What the agent said about itself, and `None` when it published nothing: an older agent has
    /// no status to read, and only then does the write clock decide the state.
    working: Option<bool>,
    stamp: i64,
}

/// Whether a person is having this conversation. A headless run, `-p` or anything through the SDK,
/// registers itself and writes a transcript exactly like a chat does, and a tool calling it in a loop
/// buries every row worth seeing. Only a terminal session declares a mode in its first entries: a
/// headless run has its prompt queued instead, carries a permission mode like a chat does, and shares
/// the same `cli` entrypoint, so the mode is what tells them apart. Asking for the interactive shape
/// leaves an unfamiliar way of starting a chat to prove itself rather than assuming it belongs.
fn interactive_chat(transcript: &Path) -> bool {
    const NEEDLE: &[u8] = br#""type":"mode""#;
    let Ok(mut file) = File::open(transcript) else {
        return false;
    };
    let mut head = vec![0u8; 16 * 1024];
    let read = file.read(&mut head).unwrap_or(0);
    head[..read].windows(NEEDLE.len()).any(|w| w == NEEDLE)
}

/// A registry file outlives its process, and a recycled pid would otherwise read as running, so
/// each entry is confirmed against the process table.
fn process_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        || std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|out| out.status.success())
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
        let dir = PathBuf::from(reg.cwd.clone().unwrap_or_default());
        if !interactive_chat(&transcript_path(&reg.session_id, &dir)) {
            continue;
        }
        out.push(Live {
            id: reg.session_id,
            dir,
            pid: reg.pid,
            // The pane reference carries window and pane after a colon; the session is the part
            // anything can be attached to.
            session: reg
                .tmux
                .filter(|t| t != "-" && !t.is_empty())
                .map(|t| t.split(':').next().unwrap_or(&t).to_string()),
            working: reg
                .status
                .as_deref()
                .filter(|s| *s != "-")
                .map(|s| WORKING.contains(&s)),
            stamp: reg.status_updated_at.unwrap_or(0) / 1000,
        });
    }
    out
}

/// The tmux session holding a conversation right now, newest activity first.
///
/// Read at the moment a row is acted on, because the drawn list is a snapshot: a conversation with
/// nothing holding it a second ago can be live now, and starting a second agent on it would put two
/// of them on one transcript.
pub fn live_session_for(id: &str) -> Option<String> {
    let sessions = crate::tmux::sessions();
    live_agents()
        .into_iter()
        .filter(|live| live.id == id)
        .filter_map(|live| live.session)
        .filter(|name| sessions.contains_key(name))
        .max_by_key(|name| sessions.get(name).map(|s| s.activity).unwrap_or(0))
}

/// Every live agent holding a conversation, which can be more than one when they run outside tmux.
pub fn pids_holding(id: &str) -> Vec<i32> {
    live_agents()
        .into_iter()
        .filter(|live| live.id == id)
        .map(|live| live.pid)
        .collect()
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
    tail_facts(transcript).0
}

/// When the conversation last did anything, from the newest entry that carries a time.
///
/// The file's own timestamp answers a different question. An agent holds its transcript open and can
/// touch it without adding an entry, which puts the file minutes old while the last thing said in it is
/// a day old, and a row claiming a chat was used minutes ago is worse than one that says nothing.
pub fn last_activity(transcript: &Path) -> Option<i64> {
    tail_facts(transcript).1
}

/// `2026-09-15T08:02:46.880Z` as seconds since the epoch. Fixed-width and always UTC, so the fields are
/// read by position and the day count comes from civil arithmetic rather than a calendar library.
fn epoch_of(stamp: &str) -> Option<i64> {
    let bytes = stamp.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let num = |a: usize, b: usize| stamp.get(a..b)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // Days from 1970-01-01, by Howard Hinnant's civil-from-days in reverse: the year starts in March so
    // a leap day lands at the end of it and needs no special case.
    let shifted_year = if m <= 2 { y - 1 } else { y };
    let era = if shifted_year >= 0 { shifted_year } else { shifted_year - 399 } / 400;
    let year_of_era = shifted_year - era * 400;
    let month_shift = if m > 2 { m - 3 } else { m + 9 };
    let day_of_year = (153 * month_shift + 2) / 5 + d - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

fn tail_facts(transcript: &Path) -> (String, Option<i64>) {
    let Ok(mut file) = File::open(transcript) else {
        return (String::new(), None);
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_WINDOW);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (String::new(), None);
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
    let mut title = String::new();
    let mut used: Option<i64> = None;
    for line in buf.lines().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if used.is_none() {
            used = value
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(epoch_of);
        }
        if title.is_empty() && value.get("type").and_then(|t| t.as_str()) == Some("ai-title") {
            if let Some(named) = value.get("aiTitle").and_then(|t| t.as_str()) {
                // Flattened for the same reason the typed text is: a newline inside a name would end
                // the row it is printed in and shift every column after it.
                let flat = named.split_whitespace().collect::<Vec<_>>().join(" ");
                if !flat.is_empty() {
                    title = flat;
                }
            }
        }
        if fallback.is_empty() && value.get("type").and_then(|t| t.as_str()) == Some("user") {
            if let Some(text) = user_text(&value) {
                fallback = text;
            }
        }
        // Both answers come from the newest entries, so there is nothing left to learn once each is
        // filled and the whole window need not be parsed.
        if !title.is_empty() && used.is_some() {
            break;
        }
    }
    let name = if title.is_empty() { fallback } else { title };
    (name, used)
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
    // Only the opening entries are read: these files reach tens of megabytes and the directory is
    // recorded from the first one.
    if let Ok(file) = File::open(transcript) {
        for line in std::io::BufReader::new(file).lines().take(200).map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(cwd) = value.get("cwd").and_then(|c| c.as_str()) {
                    return PathBuf::from(cwd);
                }
            }
        }
    }
    // Without one, the project directory's name decodes back to a path. Every `/` became a `-`, so
    // a real dash is indistinguishable, which is why the transcript is asked first.
    transcript
        .parent()
        .and_then(|p| p.file_name())
        .map(|key| PathBuf::from(key.to_string_lossy().replace('-', "/")))
        .unwrap_or_default()
}

/// Every Claude chat, live or saved. One row per conversation: several tmux sessions can hold the
/// same one, and a row each would bury every other chat under repeats of one.
pub fn chats(scope: &Scope, sessions: &HashMap<String, tmux::Session>) -> Vec<Chat> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut by_id: HashMap<String, Chat> = HashMap::new();
    let mut published_idle = false;
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
        match live.working {
            Some(true) => entry.state = State::Running,
            Some(false) => published_idle = true,
            None => {}
        }
        // What the conversation itself last recorded, and the agent's own status time only when it
        // recorded nothing: a status change is written for reasons a reader would not call use.
        entry.last_used = entry
            .last_used
            .max(last_activity(&transcript).unwrap_or(live.stamp));

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
        let session_exists = live
            .session
            .as_ref()
            .map(|name| sessions.contains_key(name))
            .unwrap_or(false);
        if session_exists && activity >= better {
            entry.session = live.session.clone();
            entry.pid = Some(live.pid);
            entry.attached = sessions
                .get(live.session.as_deref().unwrap_or_default())
                .map(|s| s.attached)
                .unwrap_or(0);
        }

        let worked = work_epoch(&transcript);
        // The write clock only decides for an agent that published nothing; overriding a published
        // `idle` would call a chat working because a file was touched.
        if entry.state != State::Running && !published_idle && now - worked <= crate::IDLE_AFTER {
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

    let cap = max_chats();
    for transcript in transcripts(scope) {
        if out.len() >= cap {
            break;
        }
        let id = transcript
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if live_ids.contains(&id) || !interactive_chat(&transcript) {
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
            last_used: last_activity(&transcript).unwrap_or_else(|| mtime(&transcript)),
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
    // Newest first, so a cap on the rows is also a cap on the transcripts opened.
    out.sort_by_key(|path| std::cmp::Reverse(mtime(path)));
    out
}
