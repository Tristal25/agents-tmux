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

## Install

```bash
git clone https://github.com/Tristal25/agents-tmux.git ~/agents-tmux
cd ~/agents-tmux && cargo build --release
cp target/release/agent-tmux ~/.local/bin/
```

It needs `tmux` and Claude Code or Codex, and nothing else at runtime: the crate is also a library, so another program can call `list()` rather than parse output.

A full list of nine chats takes about 60 ms. Most of that budget goes on the read strategy. The title a chat gave itself is the last `ai-title` line in its transcript, and those files reach tens of megabytes, so the tail is read for a few kilobytes where parsing the whole file costs all of it.

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

`Chat::action()` decides what opening a row means: `Running`, `Idle` and `Open` attach to the session holding the chat, `Exited` starts an agent on the conversation, and a live process with no session is taken over, its agent ended before the conversation reopens. `open()` reads the live state again before starting anything, since the list it came from is a snapshot. The library shells out to `tmux` for session facts and to nothing else.

## It adapts to the machine

Nothing is configured at install time. It looks at what is present and offers only that:

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

It runs on Linux and macOS. Two places where the systems disagree are handled rather than assumed: `stty` takes `-F` on GNU and `-f` on BSD, asked once at startup, and a process is checked through `/proc` where it exists and with a signal that sends nothing where it does not.

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

**Fifty chats at most, newest first.** A conversation older than that falls off the end, where the agent's own `--resume` picker still reaches it. The limit is on the reading as much as the reading matter: transcripts are opened newest first and the rest are never touched, so the time a listing takes follows this number rather than however many conversations have piled up. Against 4000 transcripts that is 100 ms with the limit and 457 ms without. `AGENT_TMUX_MAX_CHATS` sets a different one.

**Ten chats a page.** `n` and `p` turn pages, and the two new-chat rows ride along on every one. Numbers stay absolute across pages, so what you type matches what you read, and Enter takes the first chat on the page in front of you. Reading the state costs a pass over every transcript, so it happens once and pages are drawn from memory. `AGENT_TMUX_PAGE_SIZE` sets a different size; the page indicator appears once there is more than one page.

**A narrow terminal drops columns, not information.** What survives is what the choice is made on: the number, the state, and the name the chat gave itself. The rest goes in the order a reader can infer it, since the directory repeats down the list, the agent is one of two, and the age is a nicety.

| Width | Columns |
|---|---|
| 100 and up | number, agent, state, age, directory, name |
| 80 | number, state, age, directory, name |
| 64 | number, state, directory, name |
| 48 | number, state, name |
| 20 | number, name |
| 6 | number, and whatever of the name fits |

Below the state's own threshold the name keeps the space, since a number with nothing to identify it cannot be chosen on purpose, and the state is one wider window away. No drawn line exceeds the width at any size: the columns are sized to fit and the finished line is cut as well, because a window a few characters wide leaves no arithmetic that fits everything.

A resize is picked up between keypresses, so narrowing the window and widening it again leaves the list fitted to the window it is in. A listing piped to another program keeps every column whatever the terminal is doing.

**A page turn replaces the view.** The picker takes the terminal's alternate screen while it runs, so each page is drawn over the last instead of scrolling another copy into the history, and the screen you started from comes back untouched when it ends. Anything a keypress has to say, a refused page turn or an agent this machine lacks, appears inside that view and clears itself on the next draw.

**A session lives exactly as long as its chat.** tmux destroys a session when its command exits, so ending the agent removes the row. Detaching keeps it, which is the reason to run agents in tmux at all.

**The name you give a chat wins.** An agent rewrites its own name as the work changes. A rename goes in a file of its own beside the transcript. So the list shows the name you set, and the agent's newest name where you set none. That file outlives the agent, so a chat you renamed keeps that name after it exits.

**An agent is brought up to date before a chat starts it.** A running agent keeps the version it started with. The start is the one moment a new version reaches it. An executable at `$XDG_CONFIG_HOME/agent-tmux/update`, or `~/.config/agent-tmux/update`, runs first. It takes the agent's name as its argument, and prints to the terminal the chat is about to use. The chat starts once it returns, or after 90 seconds. This step runs only where that file exists, since installers differ per machine.

```bash
#!/bin/sh
# ~/.config/agent-tmux/update
case "$1" in
    claude) npm install -g @anthropic-ai/claude-code ;;
    codex) npm install -g @openai/codex ;;
esac
```

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

`contrib/cmux-report.sh` carries the rest, as escape sequences on the same stream: a desktop notification, and whether the chat is working. Register it in `~/.claude/settings.json` on the remote host:

```json
"Notification":     [{ "hooks": [{ "type": "command", "command": "~/agents-tmux/contrib/cmux-report.sh notify" }] }],
"UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "~/agents-tmux/contrib/cmux-report.sh busy"   }] }],
"Stop":             [{ "hooks": [{ "type": "command", "command": "~/agents-tmux/contrib/cmux-report.sh done"   }] }]
```

A notification is OSC 9, and working state is OSC 9;4, the progress sequence, indeterminate because a turn has no percentage to report. Nothing is configured per session and nothing has to stay alive: the bytes reach whichever terminal is attached when they are written, so a chat reattached to another window reports to that one instead.

A hook reads its payload from a pipe, so `tty` finds no terminal there; the script takes the controlling terminal by walking up to the agent that spawned it.

`contrib/cmux-remote-agent-status` carries the running state, and it runs on the *laptop*, inside the terminal that holds the connection. cmux takes agent state from a process it can trace into one of its own terminals: it reads that process's surface and binds the chat's row to it. A report sent from the remote host has no such process on this side, so it is refused at the socket:

```
ERROR: Access denied - only processes started inside cmux can connect
```

So the reporter lives inside the terminal cmux started, asks the remote host what its chat is doing every few seconds, and calls cmux's own hook commands locally. `contrib/example-remote-host-function.fish` shows the shell function that starts it beside the ssh and ends it with the shell, so nothing is configured per session and nothing has to be installed on the remote host.

```
cmux terminal
  ├─ cmux-remote-agent-status         asks the far side, reports here
  │    ├─ cmux hooks claude session-start   binds this chat's row to this surface
  │    ├─ cmux hooks claude prompt-submit   inside a turn
  │    ├─ cmux hooks claude stop            waiting for the next prompt
  │    └─ cmux set-status claude_code …     the text the row shows
  └─ ssh <host>                       the connection whose chat is being reported
       └─ tmux client → chat
```

Three states reach the row, and the text is what reads at a glance:

| Row says | The chat is | Read from |
|---|---|---|
| `Running` | inside a turn, or running a command | the agent registry's published status |
| `Needs input` | inside a turn, stopped to ask you something | the file `cmux-report.sh notify` leaves under `~/.cache/agent-tmux/waiting/` |
| `Idle` | waiting for its next prompt | the published status again |

A pending permission or question is the one state the registry cannot express, because it happens inside a turn and the chat still reads as working. The notification hook is what knows about it, so it writes a file naming the conversation and the next prompt or turn end removes it. That file outranks the published status while it exists.

`Running` and `Needs input` are written with the wording, icon and colour cmux uses for its own agents (`bolt.fill` and `bell.fill`, both `#4C8DFF`), so a chat on another machine reads exactly like one on this one. cmux draws nothing for an idle agent, so `Idle` is this tool's own: a row that goes blank between turns leaves you wondering whether the chat finished or the reporting stopped.

The text arrives through `set-status`, which draws on any row. cmux's own spinner for a running agent sits behind a feature flag that ships off, so a row shows the text whether or not that flag has been turned on.

**Pick a status key of your own.** `claude_code` belongs to cmux's agent hooks: it holds a value only while one of cmux's own agent sessions is bound to that surface, and the entry is dropped the moment none is. A key like `remote_chat` is yours and keeps whatever you set, which is why both scripts here use one.

### One process for every terminal

`contrib/cmux-remote-agent-rows` is the same idea without the per-terminal part. Row text takes a `--workspace`, so a single process can speak for every terminal open to that host, including terminals that were already open when it started:

```
tmux client on the far side ─ port ─ ssh here ─ surface ─ workspace ─ row
```

Each poll asks the host which chats its attached clients are looking at and which connection each one arrived on. The port names the ssh on this side, and the surface holding that ssh names the row to write. Run it once, from anywhere on the machine running cmux: it borrows a socket token from a terminal cmux is running, which frees it from living in one. A launch agent keeps it up:

```bash
launchctl load -w ~/Library/LaunchAgents/<your-label>.plist   # macOS notes it as a new login item
```

What it gives up is cmux's own agent lifecycle, which is accepted only from inside the surface. Use `cmux-remote-agent-status` per terminal when you want that as well; use this one when you want every row to say what its chat is doing with nothing to arrange.

**The first report has to be a session-start.** It is what binds the row to the surface, and a later event never moves that binding, so a chat reported without one keeps whichever surface it was first seen in. That surface is gone by the next connection, and a row that no longer exists shows nothing however correct the state is. The reporter therefore sends one whenever the chat it is watching changes, which also re-binds a row left pointing at an earlier connection.

**The connection settles which chat is being shown.** A chat opened in tmux keeps the environment tmux itself started with, so a marker placed in the login shell stops at the login shell, and a chat opened yesterday carries yesterday's surface id. The ssh connection is visible from both ends for as long as it lasts: the reporter reads the local port of the ssh beside it, and the remote host finds the pty holding that connection, the tmux client on that pty, and the conversation that client is looking at.

**The reporter is the process cmux watches.** cmux records the reporting process against the chat and checks it is alive before showing the chat as working. A pipeline would put a short-lived subshell in that place, which exits at once and reads as an agent that has gone, so the payload arrives on a here-string and the long-lived reporter is what cmux keeps. It lives exactly as long as the terminal holding the connection, which is the same span the chat is reachable for.

**No launch data is sent.** cmux offers to reopen a chat it has launch data for, and the command that would reopen this one runs on another machine. Leaving it out keeps cmux from starting a local chat with a conversation id that only exists on the far side.

**The row stops claiming state when the chat stops being shown.** A detached chat, an ended one, or a host that goes quiet all leave the row saying `running` forever, so two quiet answers in a row close the chat out. One is not enough: a single dropped poll would end a chat that is still working.

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

- Codex publishes no status, so a Codex row reads `Open` when a tmux session holds it and `Exited` otherwise, without splitting working from waiting. A session this tool created names the conversation in its start command, which identifies it exactly; a session running codex without that name is known only by its directory, so every rollout from that directory reads `Open` against it.
- A `No tmux` chat cannot be moved into tmux while it runs. Relocating a live process onto another terminal is what `reptyr` is for, and it declines this shape: the agent shares a process group with its launcher, and stealing the whole terminal session needs privileges it does not get. So the row ends that agent and reopens the same conversation under tmux instead.
