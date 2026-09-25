//! One chat publishing `idle` leaves another chat's own state alone.
//!
//! A chat whose agent publishes no status is judged by how recently its transcript was written. That
//! judgement belongs to each conversation, so it must hold however many other chats published
//! `idle` and in whatever order the registry lists them. This runs one real tmux server on a private
//! socket under a scratch `HOME`, with `sleep` stand-ins as the agents, and lists without opening
//! anything. It is the only test in its file because it sets process-wide variables.

use agent_tmux::{list, Scope, State};
use std::fs;
use std::process::{Child, Command};

struct Scratch {
    root: std::path::PathBuf,
    socket: std::path::PathBuf,
    agents: Vec<Child>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for agent in &mut self.agents {
            let _ = agent.kill();
            let _ = agent.wait();
        }
        let _ = Command::new("tmux").arg("-S").arg(&self.socket).arg("kill-server").output();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_silent_chat_writing_now_is_running_whatever_other_chats_published() {
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let root = std::env::temp_dir().join(format!("agent-tmux-idle-{}", std::process::id()));
    let socket_dir = root.join(format!("tmux-{}", uid.trim()));
    fs::create_dir_all(&socket_dir).unwrap();
    let socket = socket_dir.join("default");
    let mut scratch = Scratch { root: root.clone(), socket: socket.clone(), agents: Vec::new() };
    let made = Command::new("tmux")
        .arg("-S")
        .arg(&socket)
        .args(["-f", "/dev/null", "new-session", "-d", "-s", "work", "sleep 120"])
        .status()
        .unwrap();
    assert!(made.success());
    let pane = String::from_utf8(
        Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["display", "-p", "-t", "=work:", "#{session_name}:#{window_id}.#{pane_id}"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let home = root.join("home");
    fs::create_dir_all(home.join(".claude/sessions")).unwrap();
    // Nine chats publish `idle`, and two publish nothing while their transcripts are being written.
    // The registry is read in the directory's order, which may follow creation either way round, so
    // one silent chat is registered first and one last: in either order one of them comes after the
    // idle ones.
    let mut register = |n: usize, status: Option<&str>| -> String {
        let agent = Command::new("sleep").arg("120").spawn().unwrap();
        let pid = agent.id();
        scratch.agents.push(agent);
        let id = format!("00000000-0000-0000-0000-{n:012}");
        let dir = root.join(format!("project-{n}"));
        fs::create_dir_all(&dir).unwrap();
        let status = status.map(|s| format!(r#","status":"{s}","statusUpdatedAt":1"#)).unwrap_or_default();
        fs::write(
            home.join(format!(".claude/sessions/{pid}.json")),
            format!(r#"{{"pid":{pid},"sessionId":"{id}","cwd":"{}","tmux":"{pane}"{status}}}"#, dir.display()),
        )
        .unwrap();
        let project = home.join(".claude/projects").join(dir.to_string_lossy().replace('/', "-"));
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(format!("{id}.jsonl")), "{\"type\":\"mode\",\"mode\":\"normal\"}\n").unwrap();
        id
    };
    let first = register(0, None);
    for n in 1..10 {
        register(n, Some("idle"));
    }
    let last = register(10, None);

    std::env::set_var("TMUX_TMPDIR", &root);
    std::env::set_var("HOME", &home);
    std::env::remove_var("TMUX");

    let chats = list(&Scope::Everywhere);
    for silent in [&first, &last] {
        let chat = chats.iter().find(|c| &c.id == silent).expect("the silent chat is listed");
        assert_eq!(chat.state, State::Running, "its transcript was just written and it published nothing");
    }
    assert_eq!(chats.iter().filter(|c| c.state == State::Idle).count(), 9);
    drop(scratch);
}
