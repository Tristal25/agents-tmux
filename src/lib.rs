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
use std::time::{Duration, Instant};

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
    let mut out = claude::chats(scope, &sessions, &panes);
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
        // An agent already running keeps the version it started with, so the moment a chat starts one
        // is the only moment an update reaches it.
        update_agent(agent);
    }
    // The list is a snapshot. A conversation with nothing holding it when the rows were drawn can
    // be live by the time one is chosen, and starting a second agent on it puts two of them on one
    // transcript, so the live state is read again here rather than trusted.
    if let Some(id) = action.conversation() {
        if !safe_id(id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("conversation id holds characters it should not: {id}"),
            ));
        }
        if let Some((server, session)) = claude::live_session_for(id) {
            return attach(&session, Some(&server));
        }
        // An agent started outside tmux after the snapshot has no session to join. Resuming beside
        // it puts two agents on one transcript, and ending it needs the confirmation a fresh list
        // asks for, so the choice goes back to the list.
        if let Action::Resume { agent: Agent::Claude, .. } = action {
            let holding = claude::pids_holding(id);
            if !holding.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "a live agent outside tmux now holds that conversation (pid {}); list the chats again to take it over",
                        pid_list(&holding)
                    ),
                ));
            }
        }
    }
    match action {
        Action::Attach { session, server } => attach(session, server.as_deref()),
        Action::Resume { agent, id, dir } => {
            let name = tmux::free_name(agent.as_str(), dir);
            spawn(&name, resume_command(*agent, id), dir)
        }
        Action::New { agent, dir } => {
            let name = session_name(*agent, dir);
            spawn(&name, agent.as_str().to_string(), dir)
        }
        Action::OwnPicker { agent, dir } => {
            let name = session_name(*agent, dir);
            let cmd = match agent {
                Agent::Claude => "claude --resume".to_string(),
                Agent::Codex => "codex resume".to_string(),
            };
            spawn(&name, cmd, dir)
        }
        Action::Takeover {
            pid,
            agent,
            id,
            dir,
        } => {
            // A running process cannot be moved onto another terminal, so the conversation moves
            // instead: the old agent is asked to stop, then the same conversation reopens inside a
            // session. Every agent holding it stops, since one left running would share the
            // transcript with the new one.
            for holder in holders_of(id, *pid) {
                end_agent(holder);
            }
            let name = tmux::free_name(agent.as_str(), dir);
            spawn(&name, resume_command(*agent, id), dir)
        }
    }
}

/// How long an update may run before the chat starts regardless. A hook waiting on a network that
/// never answers would otherwise hold the chat for as long as that lasts.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(90);

/// Bring an agent up to date through the machine's own installer: an executable at
/// `${XDG_CONFIG_HOME:-~/.config}/agent-tmux/update`, run with the agent's name. Its result changes
/// nothing here, since a chat on the version already installed beats no chat at all.
fn update_agent(agent: Agent) {
    let Some(hook) = update_hook() else { return };
    let Ok(mut child) = Command::new(hook).arg(agent.as_str()).spawn() else {
        return;
    };
    let deadline = Instant::now() + UPDATE_TIMEOUT;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            _ => return,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn update_hook() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| claude::home().join(".config"));
    let hook = base.join("agent-tmux").join("update");
    hook.is_file().then_some(hook)
}

/// A name given on the command line wins over the derived one, since tmux takes any name and the
/// caller may want a memorable one.
fn session_name(agent: Agent, dir: &Path) -> String {
    std::env::var("AGENT_TMUX_SESSION_NAME")
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| tmux::free_name(agent.as_str(), dir))
}

/// A failed resume ends the session rather than falling back to an empty chat, which would leave a
/// session holding a conversation nobody asked for under a name that says otherwise.
/// A conversation id reaches this from a file name, and it is placed inside a single-quoted shell
/// string, so anything outside the alphabet ids use is refused rather than escaped.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn resume_command(agent: Agent, id: &str) -> String {
    let fallback =
        r#"{ printf "\ncould not open that conversation\n"; read -rsn1 -p "press any key"; }"#;
    match agent {
        Agent::Claude => format!("claude --resume '{id}' || {fallback}"),
        Agent::Codex => format!("codex resume '{id}' || {fallback}"),
    }
}

/// A chat belongs to one terminal at a time: two on one session share a single view sized to the
/// smaller of them. So a session on the user's default server moves to the terminal asking for it.
/// A session on another server belongs to the program that runs that server, so joining it shares
/// the view rather than detaching that program's own client.
fn attach(session: &str, server: Option<&Path>) -> std::io::Result<()> {
    if let Some(dir) = tmux::pane_path(server, session) {
        term::announce_dir(Path::new(&dir));
    }
    let target = format!("={session}");
    let share = server.is_some_and(|s| !tmux::is_default(s));
    let mut cmd = Command::new("tmux");
    if in_tmux() {
        let here = tmux::current_server();
        match server {
            // A client cannot switch to a session on another server, so it leaves its own server
            // and an attach to that one runs in its place.
            Some(there) if !here.as_deref().is_some_and(|h| tmux::same_socket(h, there)) => {
                let detach = if share { "" } else { " -d" };
                let join = format!(
                    "exec tmux -S {} attach{detach} -t {}",
                    shell_quote(&there.to_string_lossy()),
                    shell_quote(&target)
                );
                cmd.args(["detach-client", "-E", &join]);
            }
            // Inside tmux, switching moves the client already in use; attaching there would stack a
            // tmux inside a tmux, with two status bars and a doubled prefix key.
            _ => {
                cmd.args(["switch-client", "-t", &target]);
            }
        }
    } else {
        if let Some(there) = server {
            cmd.arg("-S").arg(there);
        }
        cmd.arg("attach");
        if !share {
            cmd.arg("-d");
        }
        cmd.args(["-t", &target]);
    }
    Err(cmd.exec())
}

/// Every agent holding a conversation, read now, plus the one a row named.
pub fn holders_of(id: &str, pid: i32) -> Vec<i32> {
    let mut holding = claude::pids_holding(id);
    if !holding.contains(&pid) {
        holding.push(pid);
    }
    holding
}

fn pid_list(pids: &[i32]) -> String {
    pids.iter().map(i32::to_string).collect::<Vec<_>>().join(", ")
}

/// One argument for `sh -c`, whatever it holds.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn spawn(session: &str, command: String, dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no such directory: {}", dir.display()),
        ));
    }
    if tmux::has_session(session) {
        return attach(session, None);
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
    // `kill` reads 0 and negative numbers as process groups, and 1 is init, so none of them names
    // an agent.
    if pid <= 1 {
        return;
    }
    // Closing a terminal ends the whole foreground group, which the agent handles cleanly, so the
    // group is the right target when the agent leads it. A group it merely belongs to can be the
    // login shell's, taking the shell and every sibling job with it, and an unreadable `ps` looks
    // the same as a group it leads. So the group is signalled only on positive proof of leadership,
    // and anything else narrows to the one process.
    match ps_field(pid, "pgid") {
        Some(g) if g == pid => signal(&format!("-{g}"), "TERM"),
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
    // A process that has already gone is the ordinary outcome here, so `kill` saying so is captured
    // rather than printed over the list.
    let _ = Command::new("kill")
        .args([&format!("-{sig}"), target])
        .output();
}

/// `/proc` answers on Linux; elsewhere the signal that asks without sending anything does.
fn alive(pid: i32) -> bool {
    // A zombie satisfies both probes below, so an unreaped agent would cost the whole wait.
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        return !stat
            .rsplit(')')
            .next()
            .map(|rest| rest.trim_start().starts_with('Z'))
            .unwrap_or(false);
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn ps_field(pid: i32, field: &str) -> Option<i32> {
    let out = Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}
