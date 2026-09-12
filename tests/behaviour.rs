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
    let c = chat(State::Exited, None);
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

/// A live row keeps its session even while a stale one names none, so the row can never point at a
/// session that has gone.
#[test]
fn a_live_state_without_a_session_still_starts_an_agent() {
    assert!(matches!(
        chat(State::Running, None).action(),
        Action::Resume { .. }
    ));
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
    drop(file);

    assert_eq!(agent_tmux::claude::title_of(&path), "what I asked for");
    std::fs::remove_file(&path).ok();
}
