# agent-tmux

One command for every coding-agent chat you have running or saved, each in its own tmux session so a dropped connection costs a reattach instead of the work.

Run it with no arguments and it prints one flat list: every live agent, every saved conversation, and a row that starts a fresh one. Pick a number.

```
 #  AGENT    STATE       LAST USED   DIRECTORY                       TASK
 1  claude   Running *   2s ago      ~/service-api                   Retry budget for the sync worker
 2  claude   Idle *      16m ago     ~/notes                         Weekly review outline
 3  claude   No tmux     4m ago      ~/scripts                       One-off log parser
 4  claude   Exited      5h ago      ~/service-api                   Token refresh race
 0  claude   New         -           ~                               empty conversation

*  another terminal is looking at this chat right now
```

## Why

Terminal multiplexers keep a long-running agent alive; nothing keeps track of *which* agent is in *which* session. After a few days you have sessions named for whatever you typed at the time, several of them holding the same conversation, and no way to tell which one is mid-task. This asks the agents themselves and lays the answer out in one table.

## Install

```bash
git clone https://github.com/Tristal25/agents-tmux.git ~/agents-tmux
ln -s ~/agents-tmux/bin/agent-tmux ~/.local/bin/agent-tmux
```

Requires `tmux`, `jq`, `bash` 4 or newer, and Claude Code or Codex. macOS ships bash 3.2, so install a current one (`brew install bash`) and it will be found ahead of the system copy.

## What each state means

| State | Meaning | Picking it |
|---|---|---|
| `Running` | a turn is in progress, or a command is executing | moves the session to your terminal |
| `Idle` | the agent is alive with a prompt waiting on you | moves the session to your terminal |
| `No tmux` | alive, started outside tmux, so no session holds it | ends that agent, reopens the conversation in a session |
| `Exited` | the conversation is saved with no agent left | starts an agent on it |
| `New` | row 0 | asks for a directory, then starts an empty chat |

`Running` and `Idle` come from the agent's own published status rather than from terminal output or file timestamps, which is what makes a chat whose subagent is working read as `Running` while its screen sits still.

## Behaviour worth knowing

**One conversation, one row, one terminal.** Two agents resumed on one conversation both append to its transcript, so a chat already running is never given a second one: choosing it moves you to the session that holds it. Opening a chat detaches whichever terminal held it, since two terminals on one session share a single view sized to the smaller window. `--share` opts into that sharing.

**Every directory, listed from anywhere.** A conversation belongs to the directory it was started in, so the list spans them all and the row carries that directory. Choosing a row creates its session there, so a chat about one project never opens in another. `--here` narrows the list to the current directory.

**Session names are derived, never asked for.** They become `<agent>-<directory>`, numbered when that name is taken. tmux needs a name to reattach by; you do not need to think about it.

**Ten chats a page.** `n` and `p` turn pages, and row 0 rides along on every one. Numbers stay absolute across pages, so what you type matches what you read, and Enter takes the first chat on the page in front of you. Reading the state costs a pass over every transcript, so it happens once and pages are drawn from memory. `AGENT_TMUX_PAGE_SIZE` sets a different size; the page indicator appears once there is more than one page.

**A session lives exactly as long as its chat.** tmux destroys a session when its command exits, so ending the agent removes the row. Detaching keeps it, which is the reason to run agents in tmux at all.

## Usage

```
agent-tmux                      the list, then act on the number you choose
agent-tmux --new                start a fresh chat here straight away
agent-tmux --new --agent codex  the same, running Codex
agent-tmux --here               narrow the list to the current directory
agent-tmux ls                   the same list, without the prompt
agent-tmux <name>               open that tmux session, or create it on the newest conversation
agent-tmux <name> <id>          create it resuming that conversation id
```

Flags work in any position: `-n/--new`, `--here`, `--agent claude|codex`, `--pick` (hand off to the agent's own picker), `--name <name>`, `--share`. `Esc` cancels any prompt.

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

- Codex support covers listing and resuming its rollouts. Its status is not published the way Claude Code's is, so a Codex row shows `Exited` until you open it.
- A `No tmux` chat cannot be moved into tmux while it runs. Relocating a live process onto another terminal is what `reptyr` is for, and it declines this shape: the agent shares a process group with its launcher, and stealing the whole terminal session needs privileges it does not get. So the row ends that agent and reopens the same conversation under tmux instead.
