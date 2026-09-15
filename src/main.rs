//! The command line over the library: print the list, or act on the row you choose.

use agent_tmux::{list, model::Action, open, Agent, Chat, Scope};
use std::io::{self, IsTerminal, Read, Write};
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
    unsafe { restore_default_sigpipe() };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut scope = Scope::Everywhere;
    let mut mode = Mode::Pick;
    let mut agent = Agent::Claude;
    let mut new = false;
    let mut pick_own = false;
    let mut name: Option<String> = None;

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
            "--pick" => pick_own = true,
            "--share" => std::env::set_var("AGENT_TMUX_SHARE", "1"),
            "--name" => name = it.next().cloned(),
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

    if new || pick_own {
        // `--pick` hands the choosing to the agent's own resume picker, which knows conversations
        // this tool has no view of, such as one belonging to another machine's directory layout.
        let action = if pick_own {
            Action::OwnPicker { agent, dir: cwd() }
        } else {
            Action::New { agent, dir: cwd() }
        };
        if let Some(session) = name {
            std::env::set_var("AGENT_TMUX_SESSION_NAME", session);
        }
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
unsafe fn restore_default_sigpipe() {
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

/// Which columns a width can afford, and how wide each one gets.
///
/// A narrow terminal gives up what a reader can infer before what it cannot. The directory repeats down
/// the list and is usually the same one; the agent is one of two, and the state already implies it for
/// a Codex row; the age is a nicety. The number, the state and the name are what the choice is actually
/// made on, so they are the last to go.
struct Layout {
    width: usize,
    agent: bool,
    age: bool,
    state: bool,
    /// Characters for the directory, or none when it is dropped.
    dir: usize,
    task: usize,
}

impl Layout {
    /// The columns other than the name, including the space that follows each.
    fn before_task(&self) -> usize {
        4 + if self.state { 12 } else { 0 }
            + if self.agent { 9 } else { 0 }
            + if self.age { 12 } else { 0 }
            + if self.dir > 0 { self.dir + 1 } else { 0 }
    }

    fn for_width(width: usize) -> Layout {
        let mut l = Layout { width, agent: true, age: true, state: true, dir: 31, task: 0 };
        // A name shorter than this stops being worth reading, so a column goes instead.
        const NAME_FLOOR: usize = 24;
        if width < l.before_task() + NAME_FLOOR {
            l.agent = false;
        }
        if width < l.before_task() + NAME_FLOOR {
            l.dir = 14;
        }
        if width < l.before_task() + NAME_FLOOR {
            l.age = false;
        }
        if width < l.before_task() + NAME_FLOOR {
            l.dir = 0;
        }
        // Down here the choice is between knowing which chat a row is and knowing what opening it
        // does. The name wins: a number with nothing to identify it cannot be chosen on purpose, and
        // the state is one keypress or one wider window away.
        if width < l.before_task() + 8 {
            l.state = false;
        }
        l.task = width.saturating_sub(l.before_task()).max(1);
        l
    }
}

/// One row of cells, padded to the layout it was given.
fn compose(l: &Layout, number: &str, agent: &str, state: &str, age: &str, dir: &str, task: &str) -> String {
    let mut line = format!(" {number:<2} ");
    if l.agent {
        line.push_str(&format!("{agent:<8} "));
    }
    if l.state {
        line.push_str(&format!("{state:<11} "));
    }
    if l.age {
        line.push_str(&format!("{age:<11} "));
    }
    if l.dir > 0 {
        let cut: String = short_to(dir, l.dir);
        line.push_str(&format!("{:<width$} ", cut, width = l.dir));
    }
    line.push_str(&task.chars().take(l.task).collect::<String>());
    // The columns are sized to fit, and this is the guarantee rather than the intention: a window a few
    // characters wide has no arithmetic that leaves room for everything, and a line over the width
    // wraps, which is what the drawing is built to avoid.
    let mut line: String = line.chars().take(l.width).collect();
    line.push('\n');
    line
}

/// A path cut from the left, since the end of it is the part that names the work.
fn short_to(dir: &str, width: usize) -> String {
    let count = dir.chars().count();
    if count <= width {
        return dir.to_string();
    }
    let tail: String = dir.chars().skip(count + 1 - width).collect();
    format!("…{tail}")
}

fn header(l: &Layout) -> String {
    compose(l, "#", "AGENT", "STATE", "LAST USED", "DIRECTORY", "TASK")
}

/// A path reads faster with the home part collapsed, and a long one is more recognisable by its tail
/// than by the root every row shares.
fn short_dir(dir: &Path) -> String {
    let text = dir.to_string_lossy().into_owned();
    // `$HOME` may be a symlink, while a chat records the resolved path.
    let home = home_resolved();
    let text = if !home.is_empty() && (text == home || text.starts_with(&format!("{home}/"))) {
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

fn row(l: &Layout, number: &str, chat: &Chat) -> String {
    // Whether a terminal is already looking at it changes what choosing it does to that terminal,
    // so it earns a mark.
    let mut state = chat.state.as_str().to_string();
    if chat.attached > 0 {
        state.push_str(" *");
    }
    let title = if chat.title.is_empty() {
        "(no title yet)".to_string()
    } else {
        chat.title.clone()
    };
    compose(
        l,
        number,
        chat.agent.as_str(),
        &state,
        &age(chat.last_used),
        &short_dir(&chat.dir),
        &title,
    )
}

/// The letter offered for each agent, fixed so the key does not move when the other agent appears.
fn letter(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "a",
        Agent::Codex => "b",
    }
}

/// Only agents this machine can start are offered, since a row for a missing one leads to a session
/// that dies at once.
fn offered() -> Vec<Agent> {
    Agent::ALL.into_iter().filter(|a| a.available()).collect()
}

fn new_rows(l: &Layout, agents: &[Agent]) -> String {
    let dir = short_dir(&cwd());
    agents
        .iter()
        .map(|a| {
            compose(
                l,
                &letter(*a).to_string(),
                a.as_str(),
                "New",
                "-",
                &dir,
                "empty conversation",
            )
        })
        .collect()
}

fn print_all(chats: &[Chat], scope: &Scope) {
    // A listing read by another program keeps every column; one read by a person is fitted to the
    // terminal in front of it.
    let l = if io::stdout().is_terminal() {
        Layout::for_width(terminal_width())
    } else {
        Layout::for_width(usize::MAX / 4)
    };
    print!("{}", header(&l));
    for (i, chat) in chats.iter().enumerate() {
        print!("{}", row(&l, &(i + 1).to_string(), chat));
    }
    print!("{}", new_rows(&l, &offered()));
    println!("\n*  another terminal is looking at this chat right now");
    match scope {
        Scope::Everywhere => println!(
            "\nEvery directory is listed. --here narrows it to {}.",
            short_dir(&cwd())
        ),
        Scope::Dir(dir) => println!("\nOnly {}. Drop --here to list every one.", short_dir(dir)),
    }
}

/// Paging replaces the view rather than adding to it, so the picker runs on the terminal's alternate
/// screen: each page is drawn over the last, and the shell's own screen comes back untouched when the
/// picker ends. Handing a row off to tmux replaces this process, which never unwinds, so leaving the
/// alternate screen happens there by hand.
fn alt_screen(on: bool) {
    if !io::stdin().is_terminal() {
        return;
    }
    // A terminal goes on reporting mouse movement for as long as some program has asked it to, and the
    // last full-screen one may have left that on. Those reports arrive as escape sequences on the same
    // stdin a keypress does, so the picker turns them off for its own screen rather than reading them.
    let seq = if on {
        "\x1b[?1049h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l"
    } else {
        "\x1b[?1049l"
    };
    print!("{seq}");
    let _ = io::stdout().flush();
}

/// Whatever the last keypress had to say about itself, shown inside the next drawing. Printed as its
/// own line it would push the view down, which is the thing being avoided.
fn draw_notice(notice: &mut String, width: usize) {
    if !notice.is_empty() {
        print!("{}", fit(&format!("\n{notice}\n"), width));
        notice.clear();
    }
}

fn pick(chats: &[Chat]) {
    let size = page_size();
    let agents = offered();
    if chats.is_empty() && agents.is_empty() {
        println!("No chats, and neither claude nor codex is on PATH.");
        return;
    }
    let mut start = 0usize;
    let mut notice = String::new();
    alt_screen(true);
    loop {
        let width = terminal_width();
        let l = Layout::for_width(width);
        if io::stdin().is_terminal() {
            print!("\x1b[H\x1b[2J");
        }
        print!("{}", header(&l));
        let last = (start + size).min(chats.len());
        for i in start..last {
            print!("{}", row(&l, &(i + 1).to_string(), &chats[i]));
        }
        print!("{}", new_rows(&l, &agents));
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
        print!("{}", fit("\n*  another terminal is looking at this chat right now\n", width));
        draw_notice(&mut notice, width);

        let default = if chats.is_empty() {
            agents.first().map(|a| letter(*a).to_string()).unwrap_or_default()
        } else {
            (start + 1).to_string()
        };
        // Each key is named with what it does, since a bare `a` reads as the word rather than as a
        // key, and the default is stated as the keypress that takes it rather than as a bracketed
        // number that leaves you to guess.
        let paging = chats.len() > size;
        let mut parts: Vec<String> = Vec::new();
        if !chats.is_empty() {
            parts.push(format!("type a number to open, enter opens {default}"));
        }
        parts.extend(
            agents
                .iter()
                .map(|a| format!("{} starts a {} chat", letter(*a), a.as_str())),
        );
        if paging {
            parts.push("n and p turn the page".to_string());
        }
        parts.push("esc quits".to_string());
        let mut prompt = format!("{} > ", parts.join(", "));

        // The same wording will not fit every window, so it gives up detail the way the columns do:
        // the keys keep their names, then lose them, and the last form is the default alone.
        if prompt.chars().count() > width {
            let mut brief: Vec<String> = Vec::new();
            if !chats.is_empty() {
                brief.push(format!("number opens, enter = {default}"));
            }
            brief.extend(agents.iter().map(|a| format!("{} = {}", letter(*a), a.as_str())));
            if paging {
                brief.push("n/p = page".to_string());
            }
            brief.push("esc = quit".to_string());
            prompt = format!("{} > ", brief.join(", "));
        }
        // Narrower again, the keys are listed without saying what they do: which keys exist is the part
        // that cannot be guessed.
        if prompt.chars().count() > width {
            let mut keys: Vec<String> = Vec::new();
            if !chats.is_empty() {
                keys.push("number".to_string());
            }
            keys.extend(agents.iter().map(|a| letter(*a).to_string()));
            if paging {
                keys.push("n/p".to_string());
            }
            keys.push("esc".to_string());
            prompt = format!("{} > ", keys.join(", "));
        }
        if prompt.chars().count() > width {
            prompt = format!("enter = {default} > ");
        }
        if prompt.chars().count() > width {
            prompt = format!("{default}> ");
        }

        let Some(choice) = read_choice(&prompt, width) else {
            alt_screen(false);
            println!("cancelled");
            return;
        };
        let reply = match choice {
            Choice::Resized => continue,
            Choice::Reply(reply) => reply,
        };
        let reply = if reply.is_empty() { default.clone() } else { reply };

        match reply.as_str() {
            "n" | "N" => {
                if start + size < chats.len() {
                    start += size;
                } else {
                    notice = "already on the last page".to_string();
                }
                continue;
            }
            "p" | "P" => {
                if start >= size {
                    start -= size;
                } else {
                    notice = "already on the first page".to_string();
                }
                continue;
            }
            "a" | "A" | "b" | "B" => {
                let wanted = if reply.eq_ignore_ascii_case("a") { Agent::Claude } else { Agent::Codex };
                if !agents.contains(&wanted) {
                    notice = format!("{} is not installed on this machine", wanted.as_str());
                    continue;
                }
                let agent = wanted;
                // The directory is the one thing worth asking about, since a new chat has no
                // history to take it from.
                alt_screen(false);
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
                    // Ending an agent mid-turn drops whatever it is working on, so a working one is
                    // confirmed rather than assumed.
                    if chat.state == agent_tmux::State::Running {
                        println!("That chat is working right now, so ending it drops what it is mid-way through.");
                        match read_text("enter ends it and reopens under tmux, esc cancels > ") {
                            None => {
                                println!("cancelled");
                                return;
                            }
                            Some(_) => {}
                        }
                    }
                    println!("ending pid {pid} so the conversation can reopen under tmux");
                }
                alt_screen(false);
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
    // Named the same way the picker's prompt is: what the key does, rather than a bracketed value that
    // leaves you to work out which key takes it.
    let width = terminal_width();
    let mut prompt = format!("directory for the new chat, enter for {} > ", short_dir(&here));
    if prompt.chars().count() > width {
        prompt = format!("directory, enter = {} > ", short_dir(&here));
    }
    if prompt.chars().count() > width {
        prompt = "directory > ".to_string();
    }
    let reply = read_text(&prompt)?;
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
                if escape_is_cancel(&mut stdin) {
                    println!();
                    drop(raw);
                    return None;
                }
            }
            // Raw mode hands these over as bytes rather than as signals, and a picker that ignores
            // them leaves the habit of pressing them doing nothing at all.
            0x03 | 0x04 => {
                println!();
                drop(raw);
                return None;
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
    print!("\r\n");
    let _ = io::stdout().flush();
    drop(raw);
    Some(typed)
}

/// The terminal's width, so a row is cut rather than wrapped. A wrapped row occupies two lines, and a
/// redraw that clears what it thinks it drew then leaves half of it behind.
fn terminal_width() -> usize {
    let from_stty = stty(&["size"])
        .and_then(|s| s.split_whitespace().nth(1).and_then(|c| c.parse::<usize>().ok()));
    let from_env = std::env::var("COLUMNS").ok().and_then(|c| c.parse::<usize>().ok());
    // A tiny terminal is still the terminal in front of someone. Only an answer too small to hold a
    // number is treated as no answer at all, since that is a reporting failure rather than a window.
    from_stty.or(from_env).filter(|w| *w >= 4).unwrap_or(120)
}

/// Cut to the width, counting characters: a title carries whatever the chat put in it, and cutting
/// bytes would split one in half.
fn fit(text: &str, width: usize) -> String {
    text.split_inclusive('\n')
        .map(|line| {
            let body = line.trim_end_matches('\n');
            let cut: String = body.chars().take(width).collect();
            if line.ends_with('\n') {
                format!("{cut}\n")
            } else {
                cut
            }
        })
        .collect()
}

/// What came back from the prompt: an answer, or the news that the terminal is a different size than
/// the drawing assumed. A resize is worth acting on without a keypress, since a list fitted to a width
/// that no longer applies stays wrong until something else happens.
enum Choice {
    Reply(String),
    Resized,
}

/// What an escape turned out to be: a cancel, or a sequence to ignore.
///
/// Arrow keys, and any other key the terminal spells as a sequence, arrive as escape then a handful of
/// bytes. Mouse reporting sends the same shape on every movement, and those carry digits: consuming a
/// fixed two bytes leaves the rest of `\x1b[<35;42;13M` to be read as an answer, so moving the mouse
/// types into the prompt and can choose a row. A sequence is therefore read to its end, which for CSI
/// and SS3 is the first byte in the final range.
fn escape_is_cancel(stdin: &mut io::Stdin) -> bool {
    let mut byte = [0u8; 1];
    if read_with_timeout(stdin, &mut byte) == 0 {
        return true;
    }
    match byte[0] {
        b'[' | b'O' => {
            // Parameters and intermediates run 0x20..0x3f; the byte that ends the sequence is above.
            while read_with_timeout(stdin, &mut byte) != 0 {
                if !(0x20..=0x3f).contains(&byte[0]) {
                    break;
                }
            }
        }
        _ => {}
    }
    false
}

/// Read one answer with escape as cancel.
///
/// A row number can run to several digits, while page turns and the new-chat rows are single letters
/// that end the answer on their own. Arrow keys also begin with escape, so a bare one counts as
/// cancel only when nothing follows it.
fn read_choice(prompt: &str, drawn_width: usize) -> Option<Choice> {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let raw = RawMode::enable();
    let mut typed = String::new();
    let mut stdin = io::stdin();
    let mut byte = [0u8; 1];
    // A terminal's reads return empty every fifth of a second rather than blocking, which is what
    // lets a resize be noticed. A pipe has no size to watch and no keys coming, so it keeps blocking
    // and an empty read there still means the stream ended.
    let watching = io::stdin().is_terminal();
    if watching {
        stty(&["min", "0", "time", "2"]);
    }
    loop {
        // Nothing typed and the stream closed means no answer at all; treating it as the default
        // would act on a row nobody chose.
        if stdin.read(&mut byte).ok()? == 0 {
            if watching {
                // Mid-answer the size is left alone: redrawing would take the half-typed number with
                // it, and the row it names is the same row at any width.
                if typed.is_empty() && terminal_width() != drawn_width {
                    drop(raw);
                    return Some(Choice::Resized);
                }
                continue;
            }
            if typed.is_empty() {
                println!();
                drop(raw);
                return None;
            }
            break;
        }
        match byte[0] {
            0x1b => {
                if escape_is_cancel(&mut stdin) {
                    println!();
                    drop(raw);
                    return None;
                }
            }
            // Raw mode hands these over as bytes rather than as signals, and a picker that ignores
            // them leaves the habit of pressing them doing nothing at all.
            0x03 | 0x04 => {
                println!();
                drop(raw);
                return None;
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
                    return Some(Choice::Reply((c as char).to_string()));
                }
            }
            _ => {}
        }
    }
    println!();
    drop(raw);
    Some(Choice::Reply(typed))
}

/// Distinguishing a lone escape from an arrow key needs a short wait, and `stty` already owns the
/// terminal settings here, so the timeout comes from the same place rather than from a dependency.
fn read_with_timeout(stdin: &mut io::Stdin, buf: &mut [u8]) -> usize {
    stty(&["time", "1", "min", "0"]);
    let read = stdin.read(buf).unwrap_or(0);
    // Back to whichever mode the caller was reading in: a terminal polls so a resize is seen, a pipe
    // blocks so an empty read still means the end of it.
    if io::stdin().is_terminal() {
        stty(&["min", "0", "time", "2"]);
    } else {
        stty(&["time", "0", "min", "1"]);
    }
    read
}

/// Which flag this `stty` takes for the terminal to act on: GNU spells it `-F`, BSD `-f`, and macOS
/// ships the BSD one. Asked once, since every later call needs the same answer.
fn stty_flag() -> &'static str {
    use std::sync::OnceLock;
    static FLAG: OnceLock<&'static str> = OnceLock::new();
    FLAG.get_or_init(|| {
        let works = Command::new("stty")
            .args(["-F", "/dev/tty", "-g"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if works {
            "-F"
        } else {
            "-f"
        }
    })
}

fn stty(args: &[&str]) -> Option<String> {
    let mut all = vec![stty_flag(), "/dev/tty"];
    all.extend_from_slice(args);
    let out = Command::new("stty").args(&all).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Raw mode, so a single keypress acts without waiting for Enter. `stty` keeps this dependency-free
/// and restores the previous settings on the way out, including on an early return.
struct RawMode {
    saved: Option<String>,
}

impl RawMode {
    fn enable() -> Self {
        let saved = stty(&["-g"]).filter(|s| !s.is_empty());
        stty(&["raw", "-echo", "min", "1", "time", "0"]);
        RawMode { saved }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        match &self.saved {
            Some(saved) => stty(&[saved]),
            None => stty(&["sane"]),
        };
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
