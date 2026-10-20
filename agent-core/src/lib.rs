// What a coding agent's session is doing, read from what the agent tells
// (its hooks' events) and what it leaves behind (its transcript, its session
// log, the project it works in). Each agent's events in Claude Code's terms,
// and what they point at; nothing of how an app shows them (Wakuwaku's
// moods and words are its own). No window, no port, no Tauri.
//
//   text      clip, base
//   project   the project a folder belongs to
//   claude    Claude Code's transcript: the session's name
//   codex     Codex's patches: the files they touch
//   agy       Antigravity's transcript: the person's words, the reply
//   opencode  OpenCode's events in Claude Code's names
//   reasonix  Reasonix's events in Claude Code's names, whose a session is
//   dsh       DeepSeek Harness: its tools in Claude Code's names, a prompt
//             as asks read one, a session's name, log, last reply, lineage
use serde_json::Value;

pub mod agy;
pub mod claude;
pub mod codex;
pub mod dsh;
pub mod opencode;
pub mod project;
pub mod reasonix;
pub mod text;

// A field's text, or "".
pub(crate) fn s<'a>(e: &'a Value, key: &str) -> &'a str {
    e.get(key).and_then(Value::as_str).unwrap_or("")
}
