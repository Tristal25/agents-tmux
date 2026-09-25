//! Opening a saved conversation refuses when a live agent outside tmux has picked it up since the
//! list was drawn.
//!
//! `open()` hands the terminal to tmux, so this runs with stub `tmux` and `claude` executables first
//! on `PATH`. The stub `tmux` answers the list queries with nothing and exits 1 on anything else, so
//! a resume that got past the check would end the test process with a failure rather than start
//! anything. The agent is a `sleep` of this test's own, under a scratch `HOME`. It is the only test in
//! its file because it sets process-wide variables.

use agent_tmux::{open, Action, Agent};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn a_resume_beside_a_live_agent_outside_tmux_is_refused() {
    let root = std::env::temp_dir().join(format!("agent-tmux-open-{}", std::process::id()));
    let bin = root.join("bin");
    let home = root.join("home");
    let dir = root.join("project");
    for d in [&bin, &home.join(".claude/sessions"), &dir] {
        fs::create_dir_all(d).unwrap();
    }
    let stub = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    stub("tmux", r#"case "$*" in *list-sessions*|*list-panes*) exit 0 ;; *has-session*) exit 1 ;; esac; echo "stub tmux: $*" >&2; exit 1"#);
    stub("claude", "exit 1");

    // Its output goes nowhere, so a resume that replaced this process could not hold the test
    // harness's pipe open for the whole sleep.
    let mut agent = Command::new("sleep")
        .arg("120")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = agent.id();
    let id = "22222222-3333-4444-5555-666666666666";
    // No `tmux` field: this agent runs outside tmux.
    fs::write(
        home.join(format!(".claude/sessions/{pid}.json")),
        format!(r#"{{"pid":{pid},"sessionId":"{id}","cwd":"{}","status":"busy","statusUpdatedAt":1}}"#, dir.display()),
    )
    .unwrap();
    let project = home.join(".claude/projects").join(dir.to_string_lossy().replace('/', "-"));
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join(format!("{id}.jsonl")), "{\"type\":\"mode\",\"mode\":\"normal\"}\n").unwrap();

    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin.clone()];
    dirs.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
    std::env::set_var("TMUX_TMPDIR", &root);
    std::env::remove_var("TMUX");

    let result = open(&Action::Resume { agent: Agent::Claude, id: id.into(), dir: dir.clone() });
    let _ = agent.kill();
    let _ = agent.wait();
    let _ = fs::remove_dir_all(&root);

    // A session no readable server holds is refused before tmux is handed the terminal.
    let away = open(&Action::Attach { session: "away".into(), server: None });
    assert_eq!(away.expect_err("an unreadable session is refused").kind(), std::io::ErrorKind::NotFound);

    let err = result.expect_err("a resume beside a live agent is refused");
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(err.to_string().contains(&pid.to_string()), "{err}");
}
