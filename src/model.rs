//! What a chat is, and the vocabulary the rest of the crate shares.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    pub const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];

    /// Whether this agent can be started here. One machine has Claude Code, another has Codex, and
    /// a third has both, so what to offer is decided by looking rather than by asking at install
    /// time: installing the other one later needs no further setup.
    pub fn available(self) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|dir| {
            let candidate = dir.join(self.as_str());
            candidate.is_file() && is_executable(&candidate)
        })
    }
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// What choosing a row does follows from its state, so the two are defined together.
///
/// `Running` and `Idle` split a live Claude agent by what it published about itself. Codex publishes
/// nothing, so a live one is `Open` and working looks the same as waiting. `NoTmux` is a live agent
/// no session holds, which cannot be attached to. `Exited` is a conversation with no agent left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Idle,
    Open,
    NoTmux,
    Exited,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Running => "Running",
            State::Idle => "Idle",
            State::Open => "Open",
            State::NoTmux => "No tmux",
            State::Exited => "Exited",
        }
    }

    /// A live agent inside a session can be joined; everything else needs a process started.
    pub fn is_live_in_tmux(self) -> bool {
        matches!(self, State::Running | State::Idle | State::Open)
    }
}

#[derive(Debug, Clone)]
pub struct Chat {
    pub agent: Agent,
    /// The conversation id, which is what resuming takes.
    pub id: String,
    pub state: State,
    /// The tmux session holding it, when one does.
    pub session: Option<String>,
    /// Terminals attached to that session.
    pub attached: u32,
    /// How many tmux sessions hold this one conversation.
    pub held: u32,
    pub pid: Option<i32>,
    /// The directory the conversation belongs to, which is where reopening it starts.
    pub dir: PathBuf,
    pub title: String,
    /// Seconds since the epoch of the last thing this conversation did.
    pub last_used: i64,
}

/// Whether the list spans every directory or only one.
#[derive(Debug, Clone)]
pub enum Scope {
    Everywhere,
    Dir(PathBuf),
}

/// What opening a chat requires of the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Move the terminal to the session already holding it.
    Attach { session: String },
    /// Start an agent on a saved conversation.
    Resume {
        agent: Agent,
        id: String,
        dir: PathBuf,
    },
    /// End a live agent no session holds, then reopen its conversation under tmux.
    Takeover {
        pid: i32,
        agent: Agent,
        id: String,
        dir: PathBuf,
    },
    /// Start an agent with nothing loaded.
    New { agent: Agent, dir: PathBuf },
    /// Start the agent on its own resume picker, which lists conversations this tool cannot see.
    OwnPicker { agent: Agent, dir: PathBuf },
}

impl Action {
    /// The conversation this action would start an agent on, if any. Attaching names no
    /// conversation, since the agent holding it is already running.
    pub fn conversation(&self) -> Option<&str> {
        match self {
            Action::Resume { id, .. } | Action::Takeover { id, .. } => Some(id),
            Action::Attach { .. } | Action::New { .. } | Action::OwnPicker { .. } => None,
        }
    }

    /// The agent this action needs installed, if it starts one at all.
    pub fn agent(&self) -> Option<Agent> {
        match self {
            Action::Attach { .. } => None,
            Action::Resume { agent, .. }
            | Action::Takeover { agent, .. }
            | Action::New { agent, .. }
            | Action::OwnPicker { agent, .. } => Some(*agent),
        }
    }
}

impl Chat {
    pub fn action(&self) -> Action {
        match (self.state, &self.session) {
            (s, Some(session)) if s.is_live_in_tmux() => Action::Attach {
                session: session.clone(),
            },
            // A live process with no session to join is taken over whatever the row says, so a
            // state that outran its session can never start a second agent on the conversation.
            _ if self.pid.is_some() && self.session.is_none() => Action::Takeover {
                pid: self.pid.unwrap_or(0),
                agent: self.agent,
                id: self.id.clone(),
                dir: self.dir.clone(),
            },
            (State::NoTmux, _) => Action::Takeover {
                pid: self.pid.unwrap_or(0),
                agent: self.agent,
                id: self.id.clone(),
                dir: self.dir.clone(),
            },
            _ => Action::Resume {
                agent: self.agent,
                id: self.id.clone(),
                dir: self.dir.clone(),
            },
        }
    }
}
