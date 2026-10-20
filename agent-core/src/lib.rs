// What a coding agent's session is doing, read from what the agent tells
// (its hooks' events) and what it leaves behind (its transcript, its session
// log, the project it works in): each agent's events as AgentEvents, in no
// agent's words (event.rs), and what they point at; nothing of how an app
// shows them (Wakuwaku's moods and words are its own). No window, no port,
// no Tauri.
//
//   event     AgentEvent: whose, which session, what it is doing
//   claude    Claude Code's events (the words the others are read in too),
//             its transcript: the session's name
//   codex     Codex's events, its patches: the files they touch
//   agy       Antigravity's events, its transcript: the person's words, the reply
//   opencode  OpenCode's events in Claude Code's names
//   omp       oh-my-pi's events, its tools by Claude Code's names
//   reasonix  Reasonix's events in Claude Code's names, whose a session is
//   dsh       DeepSeek Harness's events: its tools by Claude Code's names, a
//             prompt as an app reads one, a session's name, log, last reply,
//             lineage
//   project   the project a folder belongs to
//   text      clip, base
use std::path::Path;

use serde_json::Value;

use event::{Agent, AgentEvent};

pub mod agy;
pub mod claude;
pub mod codex;
pub mod dsh;
pub mod event;
pub mod omp;
pub mod opencode;
pub mod project;
pub mod reasonix;
pub mod text;

// What reading an event may need besides the event: the time (Reasonix
// tells a Notification from the PermissionRequest just before it by it),
// DeepSeek Harness's folder (its logs and names).
pub struct Context<'a> {
    pub now: u64,
    pub dsh_home: &'a Path,
}

// An agent's event as a session's doing, each agent's read its own way
// (OpenCode's normalized first: opencode::normalize), or None for one that
// changes nothing. Antigravity's is given the person's words and its reply
// from its transcript (with_transcript) as it is read.
pub fn read(agent: Agent, e: &mut Value, ctx: &Context) -> Option<AgentEvent> {
    match agent {
        Agent::Claude | Agent::Zcode => claude::to_event(e, agent),
        Agent::Codex => codex::to_event(e),
        Agent::Agy => agy::to_event(e),
        Agent::Opencode => opencode::to_event(e),
        Agent::Omp => omp::to_event(e),
        Agent::Reasonix => reasonix::to_event_at(e, ctx.now),
        Agent::Dsh => dsh::to_event(e, ctx.dsh_home),
    }
}

// A field's text, or "".
pub(crate) fn s<'a>(e: &'a Value, key: &str) -> &'a str {
    e.get(key).and_then(Value::as_str).unwrap_or("")
}
