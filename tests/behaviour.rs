//! What each state promises about opening it, and that a title comes out of a transcript's tail.
//!
//! These pin the behaviours worth keeping: a live chat is joined rather than started a second time,
//! a Codex conversation a session holds reads `Open`, an agent outside tmux is taken over, and the
//! newest name a chat gave itself wins over an older one and over anything typed.

use agent_tmux::model::{Action, Agent, Chat, State};
use std::io::Write;
use std::path::PathBuf;

fn chat(state: State, session: Option<&str>) -> Chat {
    Chat {
        agent: Agent::Claude,
        id: "11111111-2222-3333-4444-555555555555".into(),
        state,
        session: session.map(str::to_string),
        attached: 0,
        held: 1,
        pid: Some(4242),
        dir: PathBuf::from("/tmp"),
        title: "a task".into(),
        last_used: 0,
    }
}

#[test]
fn a_live_chat_is_joined_rather_than_started_again() {
    for state in [State::Running, State::Idle, State::Open] {
        assert_eq!(
            chat(state, Some("work")).action(),
            Action::Attach {
                session: "work".into()
            },
            "{state:?} holds a session, so it attaches"
        );
    }
}

#[test]
fn a_saved_conversation_starts_an_agent() {
    let mut c = chat(State::Exited, None);
    c.pid = None;
    assert_eq!(
        c.action(),
        Action::Resume {
            agent: Agent::Claude,
            id: c.id.clone(),
            dir: PathBuf::from("/tmp"),
        }
    );
}

#[test]
fn an_agent_outside_tmux_is_ended_before_its_conversation_reopens() {
    let c = chat(State::NoTmux, None);
    assert_eq!(
        c.action(),
        Action::Takeover {
            pid: 4242,
            agent: Agent::Claude,
            id: c.id.clone(),
            dir: PathBuf::from("/tmp"),
        }
    );
}

/// A live process with no session to join can only be taken over. Reaching `Resume` there would put
/// a second agent on a conversation one already holds, so the mapping refuses it structurally rather
/// than relying on the state having been computed correctly.
#[test]
fn a_live_process_without_a_session_is_never_resumed() {
    for state in [State::Running, State::Idle, State::Open, State::Exited] {
        assert!(
            matches!(chat(state, None).action(), Action::Takeover { .. }),
            "{state:?} with a live pid and no session must be a takeover"
        );
    }
}

/// A saved conversation has no process behind it, so it starts one.
#[test]
fn a_conversation_with_no_process_starts_an_agent() {
    let mut c = chat(State::Exited, None);
    c.pid = None;
    assert!(matches!(c.action(), Action::Resume { .. }));
}

/// Only agents this machine can start are offered, and an action naming an absent one is refused
/// rather than creating a session that dies at once.
#[test]
fn an_action_reports_which_agent_it_needs() {
    assert_eq!(
        chat(State::Running, Some("work")).action().agent(),
        None,
        "attaching needs no agent installed"
    );
    let mut c = chat(State::Exited, None);
    c.pid = None;
    assert_eq!(c.action().agent(), Some(Agent::Claude));
}

#[test]
fn the_newest_name_a_chat_gave_itself_wins() {
    let dir = std::env::temp_dir().join("agent-tmux-title-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("chat.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, r#"{{"type":"user","message":{{"content":"first thing typed"}}}}"#).unwrap();
    writeln!(file, r#"{{"type":"ai-title","aiTitle":"an early name"}}"#).unwrap();
    writeln!(file, r#"{{"type":"ai-title","aiTitle":"the current name"}}"#).unwrap();
    drop(file);

    assert_eq!(agent_tmux::claude::title_of(&path), "the current name");
    std::fs::remove_file(&path).ok();
}

/// A name the person set outranks the one the agent keeps rewriting for itself.
#[test]
fn a_renamed_chat_keeps_the_name_it_was_given() {
    let dir = std::env::temp_dir().join("agent-tmux-rename-test");
    std::fs::create_dir_all(dir.join("renamed")).unwrap();
    let path = dir.join("renamed.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, r#"{{"type":"ai-title","aiTitle":"the name it gave itself"}}"#).unwrap();
    drop(file);
    std::fs::write(
        dir.join("renamed").join("custom-title.json"),
        r#"{"customTitle":"Accessibility task"}"#,
    )
    .unwrap();

    assert_eq!(agent_tmux::claude::title_of(&path), "Accessibility task");
    std::fs::remove_dir_all(&dir).ok();
}

/// Without a name of its own, the last thing typed stands in, and the injected shapes nobody typed
/// stay out of it.
#[test]
fn an_unnamed_chat_falls_back_to_what_was_typed() {
    let dir = std::env::temp_dir().join("agent-tmux-title-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("untitled.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, r#"{{"type":"user","message":{{"content":"what I asked for"}}}}"#).unwrap();
    writeln!(file, r#"{{"type":"user","message":{{"content":"<system-reminder>noise</system-reminder>"}}}}"#).unwrap();
    // A loaded skill arrives as an injected entry, and its first line would otherwise name the chat
    // after the skill's directory.
    writeln!(file, r#"{{"type":"user","isMeta":true,"message":{{"content":[{{"type":"text","text":"Base directory for this skill: /home/me/.claude/skills/doc-writing"}}]}}}}"#).unwrap();
    drop(file);

    assert_eq!(agent_tmux::claude::title_of(&path), "what I asked for");
    std::fs::remove_file(&path).ok();
}

/// A chat is dated by what its conversation last recorded, not by the file's own timestamp: an agent
/// holds its transcript open and can touch it without adding an entry, which is what made an idle chat
/// read as used minutes ago while the last thing said in it was a day old.
#[test]
fn a_chat_is_dated_by_its_own_last_entry() {
    let dir = std::env::temp_dir().join("agent-tmux-clock-test");
    std::fs::create_dir_all(&dir).unwrap();

    let path = dir.join("dated.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, r#"{{"type":"mode","mode":"normal"}}"#).unwrap();
    writeln!(file, r#"{{"type":"user","timestamp":"2026-09-15T08:02:46.880Z"}}"#).unwrap();
    drop(file);
    assert_eq!(agent_tmux::claude::last_activity(&path), Some(1_789_459_366));

    // A leap day is the case the day arithmetic gets wrong when it treats a year as starting in
    // January, so it earns its own reading.
    let leap = dir.join("leap.jsonl");
    let mut file = std::fs::File::create(&leap).unwrap();
    writeln!(file, r#"{{"type":"assistant","timestamp":"2024-02-29T23:59:59.000Z"}}"#).unwrap();
    drop(file);
    assert_eq!(agent_tmux::claude::last_activity(&leap), Some(1_709_251_199));

    // The epoch itself, and an entry carrying no time at all.
    let epoch = dir.join("epoch.jsonl");
    let mut file = std::fs::File::create(&epoch).unwrap();
    writeln!(file, r#"{{"type":"user","timestamp":"1970-01-01T00:00:00.000Z"}}"#).unwrap();
    drop(file);
    assert_eq!(agent_tmux::claude::last_activity(&epoch), Some(0));

    let undated = dir.join("undated.jsonl");
    std::fs::write(&undated, "{\"type\":\"mode\",\"mode\":\"normal\"}\n").unwrap();
    assert_eq!(agent_tmux::claude::last_activity(&undated), None);

    std::fs::remove_dir_all(&dir).ok();
}
