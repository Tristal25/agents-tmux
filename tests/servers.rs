//! A chat on a tmux server other than the default one is found on that server.
//!
//! This runs real tmux servers, each on a private socket under a scratch `TMUX_TMPDIR`, and a scratch
//! `HOME` whose registry points at a `sleep` inside one of them. So the only processes it can reach are
//! its own. It lists and never opens, since opening attaches or ends a process for real. It is the
//! only test in its file because it sets process-wide variables.

use agent_tmux::{list, Scope, State};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch {
    root: PathBuf,
    sockets: Vec<PathBuf>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for socket in &self.sockets {
            let _ = Command::new("tmux").arg("-S").arg(socket).arg("kill-server").output();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn tmux(socket: &Path, args: &[&str]) -> String {
    let out = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(args)
        .output()
        .expect("tmux runs");
    assert!(out.status.success(), "tmux {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_chat_on_another_server_is_live_on_that_server() {
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let root = std::env::temp_dir().join(format!("agent-tmux-servers-{}", std::process::id()));
    let socket_dir = root.join(format!("tmux-{}", uid.trim()));
    fs::create_dir_all(&socket_dir).unwrap();
    let default = socket_dir.join("default");
    let other = socket_dir.join("other");
    let mut scratch = Scratch { root: root.clone(), sockets: Vec::new() };

    // Both servers hold a session named `work`, so only the server tells them apart.
    for socket in [&default, &other] {
        scratch.sockets.push(socket.clone());
        tmux(socket, &["-f", "/dev/null", "new-session", "-d", "-s", "work", "sleep 120"]);
    }
    let pid = tmux(&other, &["display", "-p", "-t", "=work:", "#{pane_pid}"]);
    let pane = tmux(&other, &["display", "-p", "-t", "=work:", "#{session_name}:#{window_id}.#{pane_id}"]);

    // A registry entry and an interactive transcript for the agent in the other server's pane.
    let home = root.join("home");
    let dir = root.join("project");
    fs::create_dir_all(home.join(".claude/sessions")).unwrap();
    fs::create_dir_all(&dir).unwrap();
    let id = "11111111-2222-3333-4444-555555555555";
    fs::write(
        home.join(format!(".claude/sessions/{pid}.json")),
        format!(
            r#"{{"pid":{pid},"sessionId":"{id}","cwd":"{}","tmux":"{pane}","status":"idle","statusUpdatedAt":1}}"#,
            dir.display()
        ),
    )
    .unwrap();
    let key = dir.to_string_lossy().replace('/', "-");
    let project = home.join(".claude/projects").join(key);
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join(format!("{id}.jsonl")), "{\"type\":\"mode\",\"mode\":\"normal\"}\n").unwrap();

    std::env::set_var("TMUX_TMPDIR", &root);
    std::env::set_var("HOME", &home);
    // Outside tmux, so the list comes from the socket directory alone.
    std::env::remove_var("TMUX");
    // Under a C locale a tmux client prints a format's tab as `_`, so the reads must not depend
    // on the locale they run in.
    std::env::set_var("LC_ALL", "C");
    for var in ["LANG", "LC_CTYPE", "LANGUAGE"] {
        std::env::remove_var(var);
    }

    let sessions = agent_tmux::tmux::sessions();
    assert_eq!(sessions.named("work").len(), 2, "both servers are read");
    assert!(agent_tmux::tmux::is_default(&default));
    assert!(!agent_tmux::tmux::is_default(&other));
    assert_eq!(
        agent_tmux::tmux::pane_path(Some(&other), "work").as_deref(),
        Some(std::env::current_dir().unwrap().to_str().unwrap()),
        "the directory of a session's pane is read on its server"
    );

    let chats = list(&Scope::Everywhere);
    let chat = chats.iter().find(|c| c.id == id).expect("the registry's chat is listed");
    assert_eq!(chat.state, State::Idle, "a chat on another server is live, not outside tmux");
    assert_eq!(chat.session.as_deref(), Some("work"));
    assert_eq!(chat.server.as_deref(), Some(other.as_path()), "the pane reference picks the server");
    drop(scratch);
}
