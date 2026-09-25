//! A hidden conversation leaves the list, and it does not use up the cap on rows.
//!
//! Saved transcripts under a scratch `HOME`, a scratch `XDG_STATE_HOME` for the hidden set, and a
//! stub `tmux` first on `PATH` that lists nothing, so no real session is read or opened. It is the
//! only test in its file because it sets process-wide variables.

use agent_tmux::{hidden, list, Scope};
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn a_hidden_chat_leaves_the_list_and_frees_its_row() {
    let root = std::env::temp_dir().join(format!("agent-tmux-hidden-{}", std::process::id()));
    let bin = root.join("bin");
    let home = root.join("home");
    let dir = root.join("project");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&dir).unwrap();
    fs::write(bin.join("tmux"), "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(bin.join("tmux"), fs::Permissions::from_mode(0o755)).unwrap();

    let project = home.join(".claude/projects").join(dir.to_string_lossy().replace('/', "-"));
    fs::create_dir_all(&project).unwrap();
    let mine = "44444444-0000-0000-0000-000000000001";
    let worker = "44444444-0000-0000-0000-000000000002";
    // The worker's transcript is the newer one, so with a cap of one row it is the one that
    // would take the row.
    for (id, stamp) in [(mine, "2026-01-01T00:00:00Z"), (worker, "2026-01-02T00:00:00Z")] {
        fs::write(
            project.join(format!("{id}.jsonl")),
            format!("{{\"type\":\"mode\",\"mode\":\"normal\",\"timestamp\":\"{stamp}\"}}\n"),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin.clone()];
    dirs.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_STATE_HOME", root.join("state"));
    std::env::set_var("TMUX_TMPDIR", &root);
    std::env::remove_var("TMUX");
    std::env::set_var("AGENT_TMUX_MAX_CHATS", "1");

    let ids = |chats: Vec<agent_tmux::Chat>| chats.into_iter().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(ids(list(&Scope::Everywhere)), vec![worker.to_string()], "the newest takes the one row");

    hidden::hide(&[worker.to_string()]).unwrap();
    hidden::hide(&[worker.to_string()]).unwrap();
    let text = fs::read_to_string(hidden::path()).unwrap();
    assert_eq!(text.lines().filter(|l| l.contains(worker)).count(), 1, "hiding twice writes one line");
    assert_eq!(ids(list(&Scope::Everywhere)), vec![mine.to_string()], "the hidden chat frees its row");

    // A comment line survives an unhide, and the chat comes back.
    fs::write(hidden::path(), format!("# started by a supervisor\n{worker}  # lane chat\n")).unwrap();
    assert!(hidden::ids().contains(worker));
    hidden::unhide(&[worker.to_string()]).unwrap();
    assert_eq!(fs::read_to_string(hidden::path()).unwrap(), "# started by a supervisor\n");
    assert_eq!(ids(list(&Scope::Everywhere)), vec![worker.to_string()]);

    assert!(hidden::hide(&["../escape".to_string()]).is_err(), "an id that is not one is refused");
    let _ = fs::remove_dir_all(&root);
}
