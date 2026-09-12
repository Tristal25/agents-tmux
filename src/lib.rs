//! One list for every running and saved coding-agent chat, each in its own tmux session.
//!
//! The library answers two questions and leaves the interface to the caller: which chats exist and
//! what state each is in ([`list`]), and what opening one requires ([`Chat::action`] and [`open`]).
//! Embedding it means calling [`list`] rather than parsing another program's output.

pub mod claude;
pub mod codex;
pub mod model;
pub mod term;
pub mod tmux;

pub use model::{Action, Agent, Chat, Scope, State};

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Whether this process runs inside tmux. An empty value counts as outside, since a shell that
/// exports the name without a value is not in a session.
pub fn in_tmux() -> bool {
    std::env::var("TMUX").map(|v| !v.is_empty()).unwrap_or(false)
}

/// A working agent writes continuously, so a conversation untouched for longer than this is waiting
/// rather than working. It only decides the state of an agent that publishes nothing itself.
pub const IDLE_AFTER: i64 = 30;

/// Every chat in scope, newest first.
pub fn list(scope: &Scope) -> Vec<Chat> {
    let sessions = tmux::sessions();
    let panes = tmux::panes();
    let mut out = claude::chats(scope, &sessions);
    out.extend(codex::chats(scope, &sessions, &panes));
    out.sort_by(|a, b| b.last_used.cmp(&a.last_used));
    out
}

/// Carry out what a row asks for.
///
/// A chat already running is never given a second agent, since two of them append to one
/// transcript: a live row joins its session, and only a conversation with nothing holding it starts
/// a process. Handing over to tmux replaces this process, so a success never returns.
pub fn open(action: &Action) -> std::io::Result<()> {
    // Starting an agent that is not installed would create a session that dies at once, so the
    // reason is reported instead. Attaching needs nothing but tmux.
    if let Some(agent) = action.agent() {
        if !agent.available() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not installed on this machine", agent.as_str()),
            ));
        }
    }
    match action {
        Action::Attach { session } => attach(session),
        Action::Resume { agent, id, dir } => {
            let name = tmux::free_name(agent.as_str(), dir);
            spawn(&name, resume_command(*agent, id), dir)
        }
        Action::New { agent, dir } => {
            let name = tmux::free_name(agent.as_str(), dir);
            spawn(&name, agent.as_str().to_string(), dir)
        }
        Action::Takeover {
            pid,
            agent,
            id,
            dir,
        } => {
            // A running process cannot be moved onto another terminal, so the conversation moves
            // instead: the old agent is asked to stop, then the same conversation reopens inside a
            // session.
            end_agent(*pid);
            let name = tmux::free_name(agent.as_str(), dir);
            spawn(&name, resume_command(*agent, id), dir)
        }
    }
}

/// A failed resume ends the session rather than falling back to an empty chat, which would leave a
/// session holding a conversation nobody asked for under a name that says otherwise.
fn resume_command(agent: Agent, id: &str) -> String {
    let fallback =
        r#"{ printf "\ncould not open that conversation\n"; read -rsn1 -p "press any key"; }"#;
    match agent {
        Agent::Claude => format!("claude --resume '{id}' || {fallback}"),
        Agent::Codex => format!("codex resume '{id}' || {fallback}"),
    }
}

/// A chat belongs to one terminal at a time: two on one session share a single view sized to the
/// smaller of them. So the session moves to the terminal asking for it.
fn attach(session: &str) -> std::io::Result<()> {
    if let Some(dir) = tmux::pane_path(session) {
        term::announce_dir(Path::new(&dir));
    }
    // Inside tmux, switching moves the client already in use; attaching there would stack a tmux
    // inside a tmux, with two status bars and a doubled prefix key.
    let args: Vec<String> = if in_tmux() {
        vec!["switch-client".into(), "-t".into(), format!("={session}")]
    } else {
        vec!["attach".into(), "-dt".into(), format!("={session}")]
    };
    Err(Command::new("tmux").args(args).exec())
}

fn spawn(session: &str, command: String, dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no such directory: {}", dir.display()),
        ));
    }
    if tmux::has_session(session) {
        return attach(session);
    }
    term::announce_dir(dir);
    let dir = dir.to_string_lossy().into_owned();
    if in_tmux() {
        let made = Command::new("tmux")
            .args(["new-session", "-d", "-s", session, "-c", &dir, &command])
            .status()?;
        if !made.success() {
            return Err(std::io::Error::other("tmux refused to create the session"));
        }
        return Err(Command::new("tmux")
            .args(["switch-client", "-t", &format!("={session}")])
            .exec());
    }
    Err(Command::new("tmux")
        .args(["new", "-s", session, "-c", &dir, &command])
        .exec())
}

/// Ask an agent to finish, and insist if it does not.
fn end_agent(pid: i32) {
    let group = ps_field(pid, "pgid");
    let session = ps_field(pid, "sid");
    // Closing a terminal ends the whole foreground group, which the agent already handles cleanly.
    // The guard keeps a login shell out of it when the agent leads its own session.
    match group {
        Some(g) if Some(g) != session => signal(&format!("-{g}"), "TERM"),
        _ => signal(&pid.to_string(), "TERM"),
    }
    for _ in 0..10 {
        if !alive(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    signal(&pid.to_string(), "KILL");
}

fn signal(target: &str, sig: &str) {
    let _ = Command::new("kill")
        .args([&format!("-{sig}"), target])
        .status();
}

/// `/proc` answers on Linux; elsewhere the signal that asks without sending anything does.
fn alive(pid: i32) -> bool {
    if PathBuf::from(format!("/proc/{pid}")).exists() {
        return true;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ps_field(pid: i32, field: &str) -> Option<i32> {
    let out = Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}
