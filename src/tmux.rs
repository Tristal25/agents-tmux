//! The one place that talks to tmux, so every other module works on parsed values.
//!
//! A machine can run several tmux servers, one per socket, and a program that needs server-wide
//! options of its own runs a server of its own. So every read walks every server in tmux's socket
//! directory, and each session and pane carries the socket of the server that holds it.

use std::io;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub name: String,
    /// The socket of the server holding it.
    pub server: PathBuf,
    /// Seconds since the epoch of the session's last activity.
    pub activity: i64,
    pub attached: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Pane {
    pub session: String,
    /// The socket of the server holding it.
    pub server: PathBuf,
    /// `session:@window.%pane`, the form an agent's registry entry records for the pane it runs in.
    pub target: String,
    pub tty: String,
    pub path: String,
    /// The command the session was created with, which names the conversation for sessions this
    /// tool created.
    pub start_command: String,
}

/// Every session on every server. A name alone does not identify a session, since two servers can
/// each hold one of the same name.
#[derive(Debug, Clone, Default)]
pub struct Sessions(pub Vec<Session>);

impl Sessions {
    pub fn get(&self, server: &Path, name: &str) -> Option<&Session> {
        self.0.iter().find(|s| s.name == name && s.server == server)
    }

    /// Every session of this name, the default server's first.
    pub fn named(&self, name: &str) -> Vec<&Session> {
        self.0.iter().filter(|s| s.name == name).collect()
    }
}

/// A tab separator keeps the fields unambiguous, since a session name may contain almost anything
/// and a path certainly can.
const SEP: char = '\t';

fn run_on(server: &Path, args: &[&str]) -> io::Result<String> {
    let out = Command::new("tmux").arg("-S").arg(server).args(args).output()?;
    // No server behind a socket is the ordinary case for a server that has exited, so it reads as
    // an empty list.
    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Where tmux keeps its sockets: `$TMUX_TMPDIR`, else `/tmp`, then one directory per user.
pub fn socket_dir() -> PathBuf {
    let base = std::env::var_os("TMUX_TMPDIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("tmux-{}", uid()))
}

fn uid() -> u32 {
    // Declared here rather than pulling in a crate for one call.
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid takes no arguments and cannot fail.
    unsafe { getuid() }
}

/// The server this process runs inside, read from `$TMUX`.
pub fn current_server() -> Option<PathBuf> {
    std::env::var("TMUX").ok().as_deref().and_then(server_in)
}

/// `$TMUX` reads `socket,pid,index`. The socket comes first and may itself hold a comma, so the
/// two fields tmux appends are taken from the end.
pub fn server_in(tmux_env: &str) -> Option<PathBuf> {
    let mut parts = tmux_env.rsplitn(3, ',');
    let (_index, _pid, socket) = (parts.next()?, parts.next()?, parts.next()?);
    (!socket.is_empty()).then(|| PathBuf::from(socket))
}

/// Two paths name one socket. `/tmp` is a link on some systems, so the resolved paths decide.
pub fn same_socket(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Whether a socket is the user's default server, the one a bare `tmux` reaches from outside tmux.
pub fn is_default(server: &Path) -> bool {
    same_socket(server, &socket_dir().join("default"))
}

/// Every server socket on the machine, the default server first. A socket whose server has exited
/// answers nothing, so it adds nothing to a list.
pub fn servers() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(socket_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_socket()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    // A server started with `-S` somewhere else is still reachable from inside it.
    if let Some(here) = current_server() {
        if !found.iter().any(|p| same_socket(p, &here)) {
            found.push(here);
        }
    }
    found.sort_by_key(|p| (p.file_name() != Some("default".as_ref()), p.clone()));
    found
}

pub fn sessions() -> Sessions {
    let fmt = format!(
        "#{{session_name}}{SEP}#{{session_activity}}{SEP}#{{session_attached}}",
        SEP = SEP
    );
    let mut out = Vec::new();
    for server in servers() {
        let text = run_on(&server, &["list-sessions", "-F", &fmt]).unwrap_or_default();
        for line in text.lines() {
            let mut it = line.split(SEP);
            let (Some(name), Some(activity), Some(attached)) = (it.next(), it.next(), it.next())
            else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            out.push(Session {
                name: name.to_string(),
                server: server.clone(),
                activity: activity.parse().unwrap_or(0),
                attached: attached.parse().unwrap_or(0),
            });
        }
    }
    Sessions(out)
}

pub fn panes() -> Vec<Pane> {
    let fmt = format!(
        "#{{pane_tty}}{SEP}#{{session_name}}{SEP}#{{window_id}}{SEP}#{{pane_id}}{SEP}#{{pane_current_path}}{SEP}#{{pane_start_command}}",
        SEP = SEP
    );
    let mut out = Vec::new();
    for server in servers() {
        let text = run_on(&server, &["list-panes", "-a", "-F", &fmt]).unwrap_or_default();
        out.extend(text.lines().filter_map(|line| {
            let mut it = line.splitn(6, SEP);
            let tty = it.next()?.to_string();
            let session = it.next()?.to_string();
            let window = it.next()?;
            let pane = it.next()?;
            Some(Pane {
                target: format!("{session}:{window}.{pane}"),
                session,
                server: server.clone(),
                tty,
                path: it.next()?.to_string(),
                start_command: it.next().unwrap_or("").to_string(),
            })
        }));
    }
    out.retain(|p| !p.tty.is_empty());
    out
}

/// Whether the server a new session would start on already holds this name. That is the current
/// server inside tmux and the default one outside it, which is where a bare `tmux` goes.
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

/// The working directory of a session's active pane, on the given server, or on the one a bare
/// `tmux` reaches when none is given.
pub fn pane_path(server: Option<&Path>, session: &str) -> Option<String> {
    // `display` takes a pane, and `=name` alone resolves no pane there: tmux prints an empty
    // format and still succeeds. The trailing colon names the session's current window and pane.
    let args = ["display", "-pt", &format!("={session}:"), "#{pane_current_path}"];
    let mut cmd = Command::new("tmux");
    if let Some(server) = server {
        cmd.arg("-S").arg(server);
    }
    let out = cmd.args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
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
