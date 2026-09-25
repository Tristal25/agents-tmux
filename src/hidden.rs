//! Conversations the list leaves out, such as the chats another agent starts for its own work.
//!
//! The hidden set is a file of conversation ids, one per line, so a program that starts a chat with
//! an id of its own choosing can hide it before the chat exists. A missing file hides nothing.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;

/// `${XDG_STATE_HOME:-~/.local/state}/agent-tmux/hidden`.
pub fn path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| crate::claude::home().join(".local/state"))
        .join("agent-tmux")
        .join("hidden")
}

/// Every hidden conversation id. A `#` starts a comment, so a line can say who hid the chat.
pub fn ids() -> HashSet<String> {
    fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split('#').next())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// Add ids to the hidden set. An id already there is left as it is.
pub fn hide(ids: &[String]) -> io::Result<()> {
    check(ids)?;
    let mut have = self::ids();
    let mut text = fs::read_to_string(path()).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for id in ids {
        if have.insert(id.clone()) {
            text.push_str(id);
            text.push('\n');
        }
    }
    write(&text)
}

/// Take ids out of the hidden set, keeping every other line as it was.
pub fn unhide(ids: &[String]) -> io::Result<()> {
    check(ids)?;
    let text = fs::read_to_string(path()).unwrap_or_default();
    let kept: String = text
        .lines()
        .filter(|line| {
            let id = line.split('#').next().unwrap_or("").trim();
            !ids.iter().any(|gone| gone == id)
        })
        .map(|line| format!("{line}\n"))
        .collect();
    write(&kept)
}

/// A conversation id is a file name, so anything outside the alphabet ids use is refused.
fn check(ids: &[String]) -> io::Result<()> {
    match ids.iter().find(|id| !crate::safe_id(id)) {
        Some(bad) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a conversation id: {bad}"),
        )),
        None => Ok(()),
    }
}

/// Written aside, then renamed over the old file, so a reader never sees half of it.
fn write(text: &str) -> io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let aside = path.with_extension("tmp");
    fs::write(&aside, text)?;
    fs::rename(&aside, &path)
}
