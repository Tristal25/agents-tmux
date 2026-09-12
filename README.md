# agent-tmux

One command for every coding-agent chat you have running or saved, each in its own tmux session so a dropped connection costs a reattach instead of the work.

Run it with no arguments and it prints one flat list: every live agent, every saved conversation, and a row that starts a fresh one. Pick a number.

```
 #  AGENT    STATE       LAST USED   DIRECTORY                       TASK
 1  claude   Running *   2s ago      ~/service-api                   Retry budget for the sync worker
 2  claude   Idle *      16m ago     ~/notes                         Weekly review outline
 3  claude   No tmux     4m ago      ~/scripts                       One-off log parser
 4  claude   Exited      5h ago      ~/service-api                   Token refresh race
 a  claude   New         -           ~                               empty conversation
 b  codex    New         -           ~                               empty conversation

*  another terminal is looking at this chat right now
```

## Why

Terminal multiplexers keep a long-running agent alive; nothing keeps track of *which* agent is in *which* session. After a few days you have sessions named for whatever you typed at the time, several of them holding the same conversation, and no way to tell which one is mid-task. This asks the agents themselves and lays the answer out in one table.

## Two implementations

The Rust crate is the one to build on: it reads the same state 17 times faster and exposes it as a library, so another program can call `list()` instead of parsing output. The shell script stays as the reference implementation, useful where a compiler is not welcome.

| | Rust | Shell |
|---|---|---|
| Command | `agent-tmux-rs` | `bin/agent-tmux` |
| A full list of 7 chats | 33–35 ms | 567–591 ms |
| Embeddable | yes, as `agent_tmux` | no |
| Needs | `tmux`, Claude Code or Codex | `tmux`, `jq`, `bash` 4+ |

The gap is mostly the read strategy. The title a chat gave itself is the last `ai-title` line in its transcript, and those files reach tens of megabytes: reading the tail costs a few kilobytes where parsing the whole file costs all of it.

## Install

Rust:

```bash
git clone https://github.com/Tristal25/agents-tmux.git ~/agents-tmux
cd ~/agents-tmux && cargo build --release
ln -s ~/agents-tmux/target/release/agent-tmux-rs ~/.local/bin/agent-tmux
```

Shell:

```bash
ln -s ~/agents-tmux/bin/agent-tmux ~/.local/bin/agent-tmux
```

The shell version needs `jq` and `bash` 4 or newer. macOS ships bash 3.2, so install a current one (`brew install bash`) and it will be found ahead of the system copy.

## Embedding it

```rust
use agent_tmux::{list, open, Scope, State};

for chat in list(&Scope::Everywhere) {
    println!("{} {:?} {}", chat.title, chat.state, chat.dir.display());
}

// Opening one replaces the process with tmux, so it returns only on failure.
if let Some(chat) = list(&Scope::Everywhere).first() {
    open(&chat.action())?;
}
```

`Chat::action()` decides what opening a row means, and the state decides the action: `Running`, `Idle` and `Open` attach to the session holding the chat, `Exited` starts an agent on the conversation, and `No tmux` ends the unreachable agent first. The library shells out to `tmux` for session facts and to nothing else.

## It adapts to the machine

Nothing is configured at install time. Both implementations look at what is present and offer only that:

| On the machine | The list offers |
|---|---|
| Claude Code and Codex | `a` and `b` |
| Claude Code only | `a` |
| Codex only | `b` |
| neither | the saved conversations, and says why nothing can be started |

The letters are fixed to their agent, so `b` stays `b` when Claude is missing and nothing moves under your fingers once the other agent appears. Asking for an absent one by flag reports it rather than creating a session that dies at once:

```
$ agent-tmux --new --agent codex
codex is not installed on this machine
```

A missing `~/.claude` or `~/.codex` is simply an empty contribution to the list, so a machine with one agent behaves as if the other never existed.

Both run on Linux and macOS. Three places where the two systems disagree are handled rather than assumed: `date -r` means a file on GNU and an epoch on BSD, so file times come from whichever `stat` answers; `stty` takes `-F` on GNU and `-f` on BSD, asked once at startup; and a process is checked through `/proc` where it exists and with a signal that sends nothing where it does not.

## What each state means

| State | Meaning | Picking it |
|---|---|---|
| `Running` | a turn is in progress, or a command is executing | moves the session to your terminal |
| `Idle` | the agent is alive with a prompt waiting on you | moves the session to your terminal |
| `No tmux` | alive, started outside tmux, so no session holds it | ends that agent, reopens the conversation in a session |
| `Open` | a Codex conversation a tmux session holds | moves that session to your terminal |
| `Exited` | the conversation is saved with no agent left | starts an agent on it |
| `New` | rows `a` and `b` | asks for a directory, then starts an empty chat on that agent |

`Running` and `Idle` come from the agent's own published status rather than from terminal output or file timestamps, which is what makes a chat whose subagent is working read as `Running` while its screen sits still.

## Behaviour worth knowing

**One conversation, one row, one terminal.** Two agents resumed on one conversation both append to its transcript, so a chat already running is never given a second one: choosing it moves you to the session that holds it. Opening a chat detaches whichever terminal held it, since two terminals on one session share a single view sized to the smaller window. `--share` opts into that sharing.

**Every directory, listed from anywhere.** A conversation belongs to the directory it was started in, so the list spans them all and the row carries that directory. Choosing a row creates its session there, so a chat about one project never opens in another. `--here` narrows the list to the current directory.

**Session names are derived, never asked for.** They become `<agent>-<directory>`, numbered when that name is taken. tmux needs a name to reattach by; you do not need to think about it.

**Ten chats a page.** `n` and `p` turn pages, and the two new-chat rows ride along on every one. Numbers stay absolute across pages, so what you type matches what you read, and Enter takes the first chat on the page in front of you. Reading the state costs a pass over every transcript, so it happens once and pages are drawn from memory. `AGENT_TMUX_PAGE_SIZE` sets a different size; the page indicator appears once there is more than one page.

**A session lives exactly as long as its chat.** tmux destroys a session when its command exits, so ending the agent removes the row. Detaching keeps it, which is the reason to run agents in tmux at all.

## Usage

```
agent-tmux                      the list; a number opens a chat, a or b starts a new one
agent-tmux --new                start a fresh chat here straight away
agent-tmux --new --agent codex  the same, running Codex
agent-tmux --here               narrow the list to the current directory
agent-tmux ls                   the same list, without the prompt
agent-tmux <name>               open that tmux session, or create it on the newest conversation
agent-tmux <name> <id>          create it resuming that conversation id
```

Flags work in any position: `-n/--new`, `--here`, `--agent claude|codex`, `--pick` (hand off to the agent's own picker), `--name <name>`, `--share`. `Esc` cancels any prompt.

## Running chats on a remote host, seen from cmux

Working from a laptop against a bigger machine leaves the chats on the far side of an ssh connection, where a terminal with a sidebar (cmux, and anything else that reads the terminal stream) labels the row with the ssh command it launched and leaves the chat behind it invisible. tmux is the reason: it consumes the inner pane's title and working directory to fill its own per-pane state, so both stop there.

Three settings and one hook close most of that gap. In `~/.tmux.conf` on the remote host:

```tmux
# Claude Code publishes the chat title as the pane title; forwarding it names the tab after the chat.
set -g set-titles on
set -g set-titles-string "#{pane_title}"

# An escape sequence a program sends for the owning terminal, such as a desktop notification,
# only gets there when tmux passes it through.
set -g allow-passthrough on
```

The working directory travels separately: `agent-tmux` emits an OSC 7 for the chat's own directory before handing the screen to tmux, so the sidebar names the folder that chat works in, whichever directory the login shell started from.

For notifications, `contrib/cmux-notify.sh` turns a Claude Code `Notification` event into an OSC 9 written to the pane's tty, wrapped in tmux passthrough. Register it in `~/.claude/settings.json` on the remote host:

```json
"Notification": [
  { "hooks": [ { "type": "command", "command": "~/agents-tmux/contrib/cmux-notify.sh", "timeout": 5 } ] }
]
```

A hook reads its payload from a pipe, so `tty` finds no terminal there; the script takes the controlling terminal from `ps` instead.

**What stays local.** A sidebar's `Running` and `Needs input` badges come from the terminal app reading `~/.claude/sessions/<pid>.json` on its own machine. A remote chat writes that file on the remote host, where those pids and socket paths mean nothing to the laptop, and no escape sequence carries the state. Notifications cross because they are part of the byte stream; the badges are not.

## How it reads the state

| Source | Used for |
|---|---|
| `~/.claude/sessions/*.json` | which conversation each running agent holds, its directory, pid, and published status |
| `~/.claude/projects/<dir key>/<id>.jsonl` | saved conversations, and the title the agent gave the task |
| `<id>/subagents/*.jsonl` | work a dispatched subagent is doing |
| `tmux list-sessions` | which session is attached, and to whom |
| `~/.codex` rollouts | Codex conversations, listed where they exist |

A registry entry outlives its process, so every pid is checked before its row is trusted.

## Limits

- Codex publishes no status, so a Codex row reads `Open` when a tmux session holds it and `Exited` otherwise, without splitting working from waiting. A session this tool created names the conversation in its start command; any other session running codex is placed by its working directory, and the newest rollout there is credited with it.
- A `No tmux` chat cannot be moved into tmux while it runs. Relocating a live process onto another terminal is what `reptyr` is for, and it declines this shape: the agent shares a process group with its launcher, and stealing the whole terminal session needs privileges it does not get. So the row ends that agent and reopens the same conversation under tmux instead.
