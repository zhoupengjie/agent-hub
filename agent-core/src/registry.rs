// The sessions an app knows, from their AgentEvents: each one's status, its
// turn, the calls running, its to-do list, the files its turn changed, how
// it ended. Nothing of how an app shows them (Wakuwaku keeps her own moods,
// what was seen, how long an ending stays): what an app would be told when
// it asks which sessions there are.
//
// As an app hears them: one event at a time, at its time. A turn is known
// by the turn id where the agent gives one (Codex's turn_id: some of its
// hooks run in the background, so a step may come after its turn ended, and
// changes nothing then but for an edit, which makes that ending one that
// changed files); a step of a turn not begun yet starts it. A subagent's
// event (within) goes only into a turn of its session's under way; it never
// starts a session, nor wakes one at rest or ended.
use std::collections::HashMap;

use serde::Serialize;

use crate::event::{Agent, AgentEvent, Call, Failure, Kind, Todo, Wait};
use crate::project::project_of;

// A session that died mid-turn never says it ended: given up after this.
pub const STALE_MS: u64 = 15 * 60 * 1000;
// A session at rest or ended, forgotten after this with nothing new.
pub const FORGET_MS: u64 = 2 * 60 * 60 * 1000;
// The turns remembered as ended, per session.
const ENDED_TURNS: usize = 4;
// Never more than this many of a list kept for a session.
const MAX_KEPT: usize = 100;

// The tools that change a file: Claude Code's, Codex's (apply_patch),
// Antigravity's (write_to_file, and its edits by the names it keeps from
// Windsurf).
pub const EDIT_TOOLS: [&str; 8] = ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch", "write_to_file", "replace_file_content", "multi_replace_file_content"];

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Status {
    // At rest: not begun, stopped, or given up.
    Idle,
    Working,
    Waiting { wait: Wait },
    // Its turn ended; `edited`: it changed files.
    Done { edited: bool },
    Failed { error: Failure },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Running {
    pub call: Call,
    pub since: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoEntry {
    pub id: String,
    pub text: String,
    pub active: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub agent: Agent,
    pub cwd: Option<String>,
    pub project: Option<String>,
    pub title: Option<String>,
    // The person's words that began its latest turn of theirs.
    pub prompt: Option<String>,
    #[serde(flatten)]
    pub status: Status,
    // When it took this status; when its work began (working or waiting);
    // how long its last turn took; when its last event came.
    pub at: u64,
    pub since: Option<u64>,
    pub took: Option<u64>,
    pub event_at: u64,
    pub turn: Option<String>,
    #[serde(skip)]
    ended_turns: Vec<String>,
    // The calls running, the latest last (subagents run theirs inside one).
    pub running: Vec<Running>,
    pub todos: Vec<TodoEntry>,
    #[serde(skip)]
    added: u32,
    // The files this turn changed.
    pub files: Vec<String>,
    #[serde(skip)]
    edited: bool,
    pub reply: Option<String>,
}

impl Session {
    fn new(id: &str, agent: Agent, now: u64) -> Session {
        Session {
            id: id.to_string(),
            agent,
            cwd: None,
            project: None,
            title: None,
            prompt: None,
            status: Status::Idle,
            at: now,
            since: None,
            took: None,
            event_at: now,
            turn: None,
            ended_turns: Vec::new(),
            running: Vec::new(),
            todos: Vec::new(),
            added: 0,
            files: Vec::new(),
            edited: false,
            reply: None,
        }
    }

    fn is_busy(&self) -> bool {
        matches!(self.status, Status::Working | Status::Waiting { .. })
    }

    fn set(&mut self, status: Status, now: u64) {
        let is_busy = matches!(status, Status::Working | Status::Waiting { .. });
        let is_ending = matches!(status, Status::Done { .. } | Status::Failed { .. });
        self.status = status;
        self.at = now;
        if is_busy {
            self.since.get_or_insert(now);
            self.took = None;
        } else {
            if let (Some(since), true) = (self.since, is_ending) {
                self.took = Some(now - since);
            }
            self.since = None;
            self.running.clear();
        }
    }

    fn apply_todo(&mut self, todo: &Todo) {
        match todo {
            Todo::Set { items } => {
                self.todos = items.iter().take(MAX_KEPT).enumerate().map(|(n, t)| TodoEntry { id: (n + 1).to_string(), text: t.text.clone(), active: t.active.clone(), status: t.status.clone() }).collect();
            }
            Todo::Add { item } => {
                self.added += 1;
                let status = if item.status.is_empty() { "pending".to_string() } else { item.status.clone() };
                if self.todos.len() < MAX_KEPT {
                    self.todos.push(TodoEntry { id: self.added.to_string(), text: item.text.clone(), active: item.active.clone(), status });
                }
            }
            Todo::Update { id, status, text, active } => {
                if status.as_deref() == Some("deleted") {
                    self.todos.retain(|t| &t.id != id);
                } else if let Some(t) = self.todos.iter_mut().find(|t| &t.id == id) {
                    for (value, field) in [(status, &mut t.status), (text, &mut t.text), (active, &mut t.active)] {
                        if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
                            *field = value.clone();
                        }
                    }
                }
            }
        }
    }
}

// What an event did to its session (SessionRegistry::apply).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Applied {
    // A session changed (or ended).
    pub changed: bool,
    // A subagent's with no turn of its session's under way: left out.
    pub left_out: bool,
    // From a turn already over: nothing but an edit counted (`changed` when
    // it made its ending one that changed files).
    pub late: bool,
    // It began a turn.
    pub began: bool,
}

#[derive(Default)]
pub struct SessionRegistry {
    sessions: HashMap<String, Session>,
}

impl SessionRegistry {
    // An event heard at `now`, and what it did.
    pub fn apply(&mut self, ev: &AgentEvent, now: u64) -> Applied {
        let status = match &ev.kind {
            Kind::SessionStart | Kind::Rest | Kind::Stopped => Status::Idle,
            Kind::TurnStart { .. } | Kind::ToolStart { .. } | Kind::ToolDone { .. } | Kind::ToolFailed { .. } | Kind::Working => Status::Working,
            Kind::Waiting { wait } => Status::Waiting { wait: wait.clone() },
            Kind::Done => Status::Done { edited: false },
            Kind::Failed { error } => Status::Failed { error: error.clone() },
            Kind::SessionEnd => Status::Idle,
            // A task of its list done says nothing of where the session is.
            Kind::TaskDone { .. } => return Applied::default(),
        };
        if ev.within && !self.sessions.get(&ev.session).is_some_and(Session::is_busy) {
            return Applied { left_out: true, ..Applied::default() };
        }
        if ev.kind == Kind::SessionEnd {
            self.sessions.remove(&ev.session);
            return Applied { changed: true, ..Applied::default() };
        }
        let s = self.sessions.entry(ev.session.clone()).or_insert_with(|| Session::new(&ev.session, ev.agent, now));
        if let Some(cwd) = &ev.cwd {
            s.project = Some(project_of(cwd));
            s.cwd = Some(cwd.clone());
        }
        s.agent = ev.agent;
        let call = match &ev.kind {
            Kind::ToolStart { call } | Kind::ToolDone { call } | Kind::ToolFailed { call, .. } => Some(call),
            _ => None,
        };
        let is_edit = matches!(ev.kind, Kind::ToolDone { .. }) && call.is_some_and(|c| EDIT_TOOLS.contains(&c.tool.as_str()));
        let turn = ev.turn.clone().unwrap_or_default();
        // Late, from a turn already over: only an edit counts, making its ending one that changed files.
        if !turn.is_empty() && s.ended_turns.contains(&turn) {
            let mut changed = false;
            if is_edit {
                if let Status::Done { edited } = &mut s.status {
                    changed = !*edited;
                    *edited = true;
                }
            }
            return Applied { changed, late: true, ..Applied::default() };
        }
        s.event_at = now;
        if let Some(title) = &ev.title {
            s.title = Some(title.clone());
        }
        if let Kind::TurnStart { prompt: Some(prompt) } = &ev.kind {
            s.prompt = Some(prompt.clone());
        }
        let ends = matches!(ev.kind, Kind::Done | Kind::Failed { .. } | Kind::Stopped);
        if !turn.is_empty() && ends {
            s.ended_turns.push(turn.clone());
            if s.ended_turns.len() > ENDED_TURNS {
                s.ended_turns.remove(0);
            }
        }
        // A turn starting anew: its start, or a step of it heard first.
        let is_new_turn = turn.is_empty() || s.turn.as_deref() != Some(turn.as_str());
        let starts = is_new_turn && (matches!(ev.kind, Kind::TurnStart { .. }) || (!turn.is_empty() && call.is_some()));
        if !turn.is_empty() {
            s.turn = Some(turn);
        }
        if starts {
            s.since = None;
            s.edited = false;
            s.running.clear();
            s.files.clear();
            s.reply = None;
            // A list all done belongs to the work before.
            if s.todos.iter().all(|t| t.status == "completed") {
                s.todos.clear();
            }
        }
        match &ev.kind {
            Kind::ToolStart { call } => {
                s.running.retain(|r| call.id.is_none() || r.call.id != call.id);
                if s.running.len() >= MAX_KEPT {
                    s.running.remove(0);
                }
                s.running.push(Running { call: call.clone(), since: now });
            }
            // Its own start, or with no id to pair them by, the latest.
            Kind::ToolDone { call } | Kind::ToolFailed { call, .. } => match s.running.iter().rposition(|r| r.call.id == call.id) {
                Some(at) => drop(s.running.remove(at)),
                None if call.id.is_none() => drop(s.running.pop()),
                None => {}
            },
            _ => {}
        }
        if is_edit {
            s.edited = true;
        }
        if let Some(file) = call.and_then(|c| c.file.as_ref()) {
            if !s.files.contains(file) && s.files.len() < MAX_KEPT {
                s.files.push(file.clone());
            }
        }
        if let Some(todo) = call.and_then(|c| c.todo.as_ref()) {
            s.apply_todo(todo);
        }
        if let Some(reply) = &ev.reply {
            s.reply = Some(reply.clone());
        }
        // An ending says whether the turn changed files (a failure leaves it
        // for an ending of the same turn after it).
        let status = match status {
            Status::Done { .. } => Status::Done { edited: std::mem::take(&mut s.edited) },
            other => other,
        };
        s.set(status, now);
        Applied { changed: true, began: starts, ..Applied::default() }
    }

    // Work that never said it ended given up, and sessions quiet too long
    // forgotten: those that changed (forgotten ones no longer there).
    pub fn check_stale(&mut self, now: u64) -> Vec<String> {
        let mut changed: Vec<String> = Vec::new();
        self.sessions.retain(|id, s| {
            let keep = !(s.status == Status::Idle && now.saturating_sub(s.at) > FORGET_MS);
            if !keep {
                changed.push(id.clone());
            }
            keep
        });
        for s in self.sessions.values_mut() {
            let age = now.saturating_sub(s.at);
            let is_ending = matches!(s.status, Status::Done { .. } | Status::Failed { .. });
            if (s.is_busy() && age > STALE_MS) || (is_ending && age > FORGET_MS) {
                s.set(Status::Idle, now);
                changed.push(s.id.clone());
            }
        }
        changed
    }

    // A session the app forgets by its own rule (Wakuwaku: an ending she
    // saw, quiet since).
    pub fn forget(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    // Every session, in no order.
    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.sessions.values()
    }

    // Every session, the latest to change first.
    pub fn list(&self) -> Vec<&Session> {
        let mut all: Vec<&Session> = self.sessions.values().collect();
        all.sort_by(|a, b| b.at.cmp(&a.at).then(a.id.cmp(&b.id)));
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{AnswerIn, What};
    use serde_json::json;

    fn ev(session: &str, kind: Kind) -> AgentEvent {
        AgentEvent { agent: Agent::Claude, session: session.into(), within: false, cwd: Some("Z:\\nowhere\\pet".into()), turn: None, title: None, reply: None, kind }
    }

    fn call(id: &str, tool: &str) -> Call {
        Call::new(id, tool, json!({}))
    }

    #[test]
    fn a_turn_that_edited_ends_with_its_files() {
        let mut r = SessionRegistry::default();
        r.apply(&ev("a", Kind::TurnStart { prompt: Some("fix it".into()) }), 1);
        r.apply(&ev("a", Kind::ToolStart { call: call("t1", "Edit") }), 2);
        assert_eq!(r.get("a").unwrap().running.len(), 1);
        r.apply(&ev("a", Kind::ToolDone { call: Call { file: Some("a.rs".into()), ..call("t1", "Edit") } }), 3);
        r.apply(&ev("a", Kind::Done), 5);
        let s = r.get("a").unwrap();
        assert_eq!((&s.status, s.took, s.files.as_slice(), s.prompt.as_deref(), s.project.as_deref()), (&Status::Done { edited: true }, Some(4), &["a.rs".to_string()][..], Some("fix it"), Some("pet")));
        assert!(s.running.is_empty());
        // The next turn starts clean.
        r.apply(&ev("a", Kind::TurnStart { prompt: None }), 6);
        let s = r.get("a").unwrap();
        assert_eq!((&s.status, s.files.len(), s.prompt.as_deref()), (&Status::Working, 0, Some("fix it")));
    }

    #[test]
    fn a_late_edit_after_its_turn_ended() {
        let mut r = SessionRegistry::default();
        let codex = |turn: &str, kind| AgentEvent { agent: Agent::Codex, turn: Some(turn.into()), ..ev("c", kind) };
        assert_eq!(r.apply(&codex("t1", Kind::TurnStart { prompt: None }), 1), Applied { changed: true, began: true, ..Applied::default() });
        r.apply(&codex("t1", Kind::Done), 2);
        assert_eq!(r.get("c").unwrap().status, Status::Done { edited: false });
        // A step after its turn ended changes nothing; an edit makes it one that changed files.
        assert_eq!(r.apply(&codex("t1", Kind::ToolStart { call: call("x", "Bash") }), 3), Applied { late: true, ..Applied::default() });
        assert_eq!(r.get("c").unwrap().status, Status::Done { edited: false });
        assert_eq!(r.apply(&codex("t1", Kind::ToolDone { call: call("y", "apply_patch") }), 4), Applied { changed: true, late: true, ..Applied::default() });
        assert_eq!(r.get("c").unwrap().status, Status::Done { edited: true });
        assert!(!r.apply(&codex("t1", Kind::ToolDone { call: call("y", "apply_patch") }), 4).changed);
        // A turn heard first by a step starts anew.
        assert!(r.apply(&codex("t2", Kind::ToolStart { call: call("z", "Bash") }), 5).began);
        assert_eq!((r.get("c").unwrap().status.clone(), r.get("c").unwrap().turn.as_deref()), (Status::Working, Some("t2")));
    }

    #[test]
    fn a_subagent_s_steps_go_only_into_a_turn_under_way() {
        let mut r = SessionRegistry::default();
        let within = |kind| AgentEvent { within: true, ..ev("a", kind) };
        assert!(r.apply(&within(Kind::ToolDone { call: call("c1", "Write") }), 1).left_out);
        assert!(r.get("a").is_none());
        r.apply(&ev("a", Kind::TurnStart { prompt: None }), 2);
        assert!(r.apply(&within(Kind::ToolDone { call: Call { file: Some("n.md".into()), ..call("c1", "Write") } }), 3).changed);
        r.apply(&ev("a", Kind::Done), 4);
        assert_eq!(r.get("a").unwrap().status, Status::Done { edited: true });
        assert!(r.apply(&within(Kind::Working), 5).left_out);
    }

    #[test]
    fn waiting_to_do_lists_and_the_end() {
        let mut r = SessionRegistry::default();
        let approval = Wait::Approval { what: What::Call { call: call("", "Bash") }, answer_in: AnswerIn::Here };
        r.apply(&ev("a", Kind::Waiting { wait: approval.clone() }), 1);
        assert_eq!(r.get("a").unwrap().status, Status::Waiting { wait: approval });
        let add = |text: &str| Kind::ToolStart { call: Call { todo: Some(Todo::Add { item: crate::event::TodoItem { text: text.into(), active: String::new(), status: String::new() } }), ..call("", "TaskCreate") } };
        r.apply(&ev("a", add("A")), 2);
        r.apply(&ev("a", add("B")), 3);
        r.apply(&ev("a", Kind::ToolStart { call: Call { todo: Some(Todo::Update { id: "1".into(), status: Some("completed".into()), text: None, active: None }), ..call("", "TaskUpdate") } }), 4);
        let todos: Vec<(&str, &str)> = r.get("a").unwrap().todos.iter().map(|t| (t.text.as_str(), t.status.as_str())).collect();
        assert_eq!(todos, [("A", "completed"), ("B", "pending")]);
        r.apply(&ev("a", Kind::SessionEnd), 5);
        assert!(r.list().is_empty());
    }

    #[test]
    fn stale_work_gives_up_and_quiet_sessions_are_forgotten() {
        let mut r = SessionRegistry::default();
        r.apply(&ev("a", Kind::Working), 0);
        r.apply(&ev("b", Kind::SessionStart), 0);
        assert_eq!(r.check_stale(STALE_MS + 1), ["a"]);
        assert_eq!(r.get("a").unwrap().status, Status::Idle);
        assert_eq!(r.check_stale(FORGET_MS + 1), ["b"]);
        assert_eq!(r.list().len(), 1);
        r.forget("a");
        assert!(r.sessions().next().is_none());
    }
}
