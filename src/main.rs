//! The command line over the library: print the list, or act on the row you choose.

use agent_tmux::{list, model::Action, open, Agent, Chat, Scope};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// How many chats a page shows. Reading the state costs a pass over the transcripts, so it happens
/// once and the pages are drawn from memory.
fn page_size() -> usize {
    std::env::var("AGENT_TMUX_PAGE_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(10)
}

fn main() {
    // Piping the list into something that stops reading early, `head` for instance, closes stdout
    // underneath us. That is an ordinary way to use a list, so the write simply ends the program.
    unsafe { libc_signal_ignore_sigpipe() };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut scope = Scope::Everywhere;
    let mut mode = Mode::Pick;
    let mut agent = Agent::Claude;
    let mut new = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => {
                print!("{}", usage());
                return;
            }
            "ls" | "list" => mode = Mode::List,
            "pick" | "select" => mode = Mode::Pick,
            "--here" | "--cwd" => scope = Scope::Dir(cwd()),
            "-a" | "--all" => scope = Scope::Everywhere,
            "-n" | "--new" => new = true,
            "--codex" => agent = Agent::Codex,
            "--agent" => match it.next().map(String::as_str) {
                Some("claude") => agent = Agent::Claude,
                Some("codex") => agent = Agent::Codex,
                other => {
                    eprintln!("agent must be claude or codex: {}", other.unwrap_or(""));
                    std::process::exit(2);
                }
            },
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    if new {
        let action = Action::New {
            agent,
            dir: cwd(),
        };
        if let Err(err) = open(&action) {
            eprintln!("{err}");
            std::process::exit(1);
        }
        return;
    }

    let chats = list(&scope);
    match mode {
        Mode::List => print_all(&chats, &scope),
        Mode::Pick => pick(&chats),
    }
}

enum Mode {
    List,
    Pick,
}

/// Restore the default SIGPIPE behaviour, which Rust disables at startup so that a closed pipe
/// surfaces as an error instead of ending the process.
unsafe fn libc_signal_ignore_sigpipe() {
    // SIG_DFL for SIGPIPE, declared here rather than pulling in a crate for two constants.
    extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    signal(SIGPIPE, SIG_DFL);
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

fn header() -> String {
    format!(
        " {:<2} {:<8} {:<11} {:<11} {:<31} {}\n",
        "#", "AGENT", "STATE", "LAST USED", "DIRECTORY", "TASK"
    )
}

/// A path reads faster with the home part collapsed, and a long one is more recognisable by its tail
/// than by the root every row shares.
fn short_dir(dir: &Path) -> String {
    let text = dir.to_string_lossy().into_owned();
    // `$HOME` may be a symlink, while a chat records the resolved path.
    let home = home_resolved();
    let text = if !home.is_empty() && text.starts_with(&home) {
        format!("~{}", &text[home.len()..])
    } else {
        text
    };
    if text.chars().count() <= 30 {
        return text;
    }
    let tail: String = text.chars().rev().take(29).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

fn home_resolved() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home)
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(home)
}

fn age(seconds: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = now - seconds;
    match d {
        d if d < 0 => "just now".into(),
        d if d < 60 => format!("{d}s ago"),
        d if d < 3600 => format!("{}m ago", d / 60),
        d if d < 86400 => format!("{}h ago", d / 3600),
        d => format!("{}d ago", d / 86400),
    }
}

fn row(number: &str, chat: &Chat) -> String {
    // Whether a terminal is already looking at it changes what choosing it does to that terminal,
    // so it earns a mark.
    let mut state = chat.state.as_str().to_string();
    if chat.attached > 0 {
        state.push_str(" *");
    }
    let title = if chat.title.is_empty() {
        "(no title yet)".to_string()
    } else {
        chat.title.chars().take(52).collect()
    };
    format!(
        " {:<2} {:<8} {:<11} {:<11} {:<31} {}\n",
        number,
        chat.agent.as_str(),
        state,
        age(chat.last_used),
        short_dir(&chat.dir),
        title
    )
}

fn new_rows() -> String {
    let dir = short_dir(&cwd());
    format!(
        " {:<2} {:<8} {:<11} {:<11} {:<31} {}\n {:<2} {:<8} {:<11} {:<11} {:<31} {}\n",
        "a", "claude", "New", "-", dir, "empty conversation",
        "b", "codex", "New", "-", dir, "empty conversation"
    )
}

fn print_all(chats: &[Chat], scope: &Scope) {
    print!("{}", header());
    for (i, chat) in chats.iter().enumerate() {
        print!("{}", row(&(i + 1).to_string(), chat));
    }
    print!("{}", new_rows());
    println!("\n*  another terminal is looking at this chat right now");
    match scope {
        Scope::Everywhere => println!(
            "\nEvery directory is listed. --here narrows it to {}.",
            short_dir(&cwd())
        ),
        Scope::Dir(dir) => println!("\nOnly {}. Drop --here to list every one.", short_dir(dir)),
    }
}

fn pick(chats: &[Chat]) {
    let size = page_size();
    let mut start = 0usize;
    loop {
        print!("{}", header());
        let last = (start + size).min(chats.len());
        for i in start..last {
            print!("{}", row(&(i + 1).to_string(), &chats[i]));
        }
        print!("{}", new_rows());
        if chats.len() > size {
            println!(
                "\nchats {}-{} of {}, page {}/{}",
                start + 1,
                last,
                chats.len(),
                start / size + 1,
                (chats.len() + size - 1) / size
            );
        }
        println!("\n*  another terminal is looking at this chat right now");

        let default = if chats.is_empty() { "a".to_string() } else { (start + 1).to_string() };
        let mut prompt = format!("Choice [{default}], a new claude chat, b new codex chat");
        if chats.len() > size {
            prompt.push_str(", n/p page");
        }
        prompt.push_str(" (esc to quit): ");

        let Some(reply) = read_choice(&prompt) else {
            println!("cancelled");
            return;
        };
        let reply = if reply.is_empty() { default.clone() } else { reply };

        match reply.as_str() {
            "n" | "N" => {
                if start + size < chats.len() {
                    start += size;
                } else {
                    println!("already on the last page");
                }
                println!();
                continue;
            }
            "p" | "P" => {
                if start >= size {
                    start -= size;
                } else {
                    println!("already on the first page");
                }
                println!();
                continue;
            }
            "a" | "A" | "b" | "B" => {
                let agent = if reply.eq_ignore_ascii_case("a") { Agent::Claude } else { Agent::Codex };
                // The directory is the one thing worth asking about, since a new chat has no
                // history to take it from.
                let Some(dir) = ask_dir() else {
                    println!("cancelled");
                    return;
                };
                act(&Action::New { agent, dir });
                return;
            }
            _ => {}
        }

        match reply.parse::<usize>() {
            Ok(n) if n >= 1 && n <= chats.len() => {
                let chat = &chats[n - 1];
                if let Action::Takeover { pid, .. } = chat.action() {
                    println!("ending pid {pid} so the conversation can reopen under tmux");
                }
                act(&chat.action());
                return;
            }
            _ => println!("Out of range: {reply}\n"),
        }
    }
}

fn act(action: &Action) {
    if let Err(err) = open(action) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

/// Asks where a new chat should work, defaulting to where the list was run from. Claude keys its
/// history off the directory string, so the answer is resolved to its real path: `~/x` and
/// `/home/you/x` would otherwise file one directory under two names.
fn ask_dir() -> Option<PathBuf> {
    let here = cwd();
    let reply = read_text(&format!("directory [{}] (esc to quit): ", short_dir(&here)))?;
    if reply.is_empty() {
        return Some(here);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = if reply == "~" {
        PathBuf::from(&home)
    } else if let Some(rest) = reply.strip_prefix("~/") {
        PathBuf::from(&home).join(rest)
    } else if reply.starts_with('/') {
        PathBuf::from(&reply)
    } else {
        here.join(&reply)
    };
    match expanded.canonicalize() {
        Ok(dir) => Some(dir),
        Err(_) => {
            println!("no such directory: {reply}");
            None
        }
    }
}

/// Read a line of text with escape as cancel, for an answer that is not one of the offered keys.
fn read_text(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let raw = RawMode::enable();
    let mut typed = String::new();
    let mut stdin = io::stdin();
    let mut byte = [0u8; 1];
    while stdin.read(&mut byte).unwrap_or(0) > 0 {
        match byte[0] {
            0x1b => {
                let mut rest = [0u8; 2];
                if read_with_timeout(&mut stdin, &mut rest) == 0 {
                    println!();
                    drop(raw);
                    return None;
                }
            }
            b'\n' | b'\r' => break,
            0x7f | 0x08 => {
                if typed.pop().is_some() {
                    print!("\u{8} \u{8}");
                    let _ = io::stdout().flush();
                }
            }
            c if c.is_ascii_graphic() || c == b' ' => {
                typed.push(c as char);
                print!("{}", c as char);
                let _ = io::stdout().flush();
            }
            _ => {}
        }
    }
    println!();
    drop(raw);
    Some(typed)
}

/// Read one answer with escape as cancel.
///
/// A row number can run to several digits, while page turns and the new-chat rows are single letters
/// that end the answer on their own. Arrow keys also begin with escape, so a bare one counts as
/// cancel only when nothing follows it.
fn read_choice(prompt: &str) -> Option<String> {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let raw = RawMode::enable();
    let mut typed = String::new();
    let mut stdin = io::stdin();
    let mut byte = [0u8; 1];
    loop {
        if stdin.read(&mut byte).ok()? == 0 {
            break;
        }
        match byte[0] {
            0x1b => {
                let mut rest = [0u8; 2];
                // A lone escape arrives with nothing behind it; an arrow key brings two more bytes.
                if read_with_timeout(&mut stdin, &mut rest) == 0 {
                    println!();
                    drop(raw);
                    return None;
                }
            }
            b'\n' | b'\r' => break,
            0x7f | 0x08 => {
                if typed.pop().is_some() {
                    print!("\u{8} \u{8}");
                    let _ = io::stdout().flush();
                }
            }
            c @ b'0'..=b'9' => {
                typed.push(c as char);
                print!("{}", c as char);
                let _ = io::stdout().flush();
            }
            c @ (b'n' | b'N' | b'p' | b'P' | b'a' | b'A' | b'b' | b'B') => {
                if typed.is_empty() {
                    println!("{}", c as char);
                    drop(raw);
                    return Some((c as char).to_string());
                }
            }
            _ => {}
        }
    }
    println!();
    drop(raw);
    Some(typed)
}

/// Distinguishing a lone escape from an arrow key needs a short wait, and `stty` already owns the
/// terminal settings here, so the timeout comes from the same place rather than from a dependency.
fn read_with_timeout(stdin: &mut io::Stdin, buf: &mut [u8]) -> usize {
    let _ = Command::new("stty").args(["-F", "/dev/tty", "time", "1", "min", "0"]).status();
    let read = stdin.read(buf).unwrap_or(0);
    let _ = Command::new("stty").args(["-F", "/dev/tty", "time", "0", "min", "1"]).status();
    read
}

/// Raw mode, so a single keypress acts without waiting for Enter. `stty` keeps this dependency-free
/// and restores the previous settings on the way out, including on an early return.
struct RawMode {
    saved: Option<String>,
}

impl RawMode {
    fn enable() -> Self {
        let saved = Command::new("stty")
            .args(["-F", "/dev/tty", "-g"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        let _ = Command::new("stty")
            .args(["-F", "/dev/tty", "raw", "-echo", "min", "1", "time", "0"])
            .status();
        RawMode { saved }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some(saved) = &self.saved {
            let _ = Command::new("stty").args(["-F", "/dev/tty", saved]).status();
        } else {
            let _ = Command::new("stty").args(["-F", "/dev/tty", "sane"]).status();
        }
    }
}

fn usage() -> String {
    "\
Usage:
  agent-tmux                      every chat, from every directory; a number opens one,
                                  a starts a new Claude chat, b a new Codex one
  agent-tmux --new                start a fresh chat here straight away
  agent-tmux --new --agent codex  the same, running Codex
  agent-tmux --here               narrow the list to the current directory
  agent-tmux ls                   the same list, without the prompt

States: Running (a turn or command in flight), Idle (alive, waiting on you), Open (a Codex
conversation a session holds, which is as far as Codex state goes), No tmux (alive but outside tmux,
so choosing it ends that agent and reopens the conversation in a session), Exited (the conversation
is saved with no agent left). A `*` marks a chat another terminal is looking at right now.

Choosing a row moves you into that conversation's own directory. tmux session names are derived
from the directory and never asked for.
"
    .to_string()
}
