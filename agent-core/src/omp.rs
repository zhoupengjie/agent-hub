// oh-my-pi (omp) events as a session's doing. omp has no hooks of its own
// to run: an extension of an app's inside it hands each event on in Claude
// Code's shape, so most are read as Claude Code's (claude.rs). What differs:
// omp's tools are named in lower case, their file argument `path`, and each
// tool call comes with its intent, the model's words for what it does
// ("Reading a.txt"), for the tools that have no words of an app's. And a
// few of its own: Interrupt (the person stopped the turn),
// ApprovalRequested and ApprovalResolved (omp asks for a tool in its own
// window, which no app can answer: waiting there, and back), and a plan
// review answered (PostToolUse for ExitPlanMode with the choice: a turn
// follows, or none). As omp 18.11.1 sends them (2026-10-15).
use serde_json::{json, Value};

use crate::claude::{bare, to_event as claude_event};
use crate::event::{given, Agent, AgentEvent, AnswerIn, Call, Kind, Wait, What};
use crate::s;
use crate::text::clip;

// The first of these arguments given.
fn first<'a>(input: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter().map(|k| s(input, k)).find(|v| !v.trim().is_empty()).unwrap_or("")
}

// An omp tool by Claude Code's name, with the arguments Claude Code's tool
// has, or None for one of its own (lsp, python, browser, eval, …).
pub fn as_claude(tool: &str, input: &Value, intent: &str) -> Option<(&'static str, Value)> {
    let path = || first(input, &["path", "file_path"]);
    Some(match tool {
        "bash" => ("Bash", json!({ "command": s(input, "command"), "description": intent })),
        "read" => ("Read", json!({ "file_path": path() })),
        "write" => ("Write", json!({ "file_path": path() })),
        "edit" | "ast_edit" => ("Edit", json!({ "file_path": path() })),
        "notebook" => ("NotebookEdit", json!({ "notebook_path": path() })),
        "grep" | "ast_grep" => ("Grep", json!({ "pattern": first(input, &["pattern", "query"]) })),
        // glob takes its pattern as `path` ("*.txt").
        "glob" | "find" => ("Glob", json!({ "pattern": first(input, &["pattern", "path", "query"]) })),
        "task" => {
            let job = input.get("tasks").and_then(Value::as_array).and_then(|t| t.first()).unwrap_or(&Value::Null);
            let what = if intent.is_empty() { first(job, &["name", "description"]) } else { intent };
            ("Task", json!({ "description": what, "subagent_type": s(job, "agent") }))
        }
        "ask" => ("AskUserQuestion", json!({})),
        "todo" => ("TodoWrite", json!({})),
        "web_search" => ("WebSearch", json!({ "query": s(input, "query") })),
        "fetch" => {
            let item = input.get("items").and_then(Value::as_array).and_then(|i| i.first()).unwrap_or(&Value::Null);
            ("WebFetch", json!({ "url": if s(input, "url").is_empty() { s(item, "url") } else { s(input, "url") } }))
        }
        _ => return None,
    })
}

// The event with omp's tool as Claude Code's (its own kept as it is).
fn translated(e: &Value) -> Value {
    let mut e = e.clone();
    let (tool, intent) = (s(&e, "tool_name").to_string(), s(&e, "intent").to_string());
    let input = e.get("tool_input").cloned().unwrap_or(Value::Null);
    if let Some((name, args)) = as_claude(&tool, &input, &intent) {
        e["tool_name"] = json!(name);
        e["tool_input"] = args;
    }
    e
}

// A call by Claude Code's name where it has one, omp's own name, input and
// intent kept.
fn call_of(e: &Value) -> Call {
    let (tool, intent) = (s(e, "tool_name"), s(e, "intent"));
    let input = e.get("tool_input").cloned().unwrap_or(Value::Null);
    let mut call = match as_claude(tool, &input, intent) {
        Some((name, args)) => Call { own: Some(tool.to_string()), raw: Some(input), ..Call::new(s(e, "tool_use_id"), name, args) },
        None => Call::new(s(e, "tool_use_id"), tool, input),
    };
    call.intent = given(intent);
    call
}

// An omp event as a session's doing, or None for one that changes nothing.
pub fn to_event(e: &Value) -> Option<AgentEvent> {
    if !e.is_object() {
        return None;
    }
    let (name, tool) = (s(e, "hook_event_name"), s(e, "tool_name"));
    let own = |kind| bare(Agent::Omp, s(e, "session_id"), s(e, "cwd"), kind);
    let mut event = match name {
        "Interrupt" => own(Kind::Stopped),
        // Asked in omp's own window, which no app can answer.
        "ApprovalRequested" => own(Kind::Waiting { wait: Wait::Approval { what: What::Call { call: call_of(e) }, answer_in: AnswerIn::Terminal } }),
        "ApprovalResolved" => own(Kind::Working),
        // A plan review answered: a turn follows (executed, refined), or
        // none (saved, put away).
        "PostToolUse" if tool == "ExitPlanMode" => own(if matches!(s(e, "choice"), "execute" | "compact" | "keep" | "refine") { Kind::Working } else { Kind::Rest }),
        _ => {
            let mut event = claude_event(&translated(e), Agent::Omp)?;
            let ours = call_of(e);
            match &mut event.kind {
                // Its step in its own words where it has them.
                Kind::ToolStart { call } | Kind::ToolDone { call } | Kind::ToolFailed { call, .. } => {
                    call.own = ours.own;
                    call.raw = ours.raw;
                    call.intent = ours.intent;
                }
                _ => {}
            }
            // The file an edit changed, as omp resolved it (a whole path).
            if let Kind::ToolDone { call } = &mut event.kind {
                if call.file.is_some() && !s(e, "file").is_empty() {
                    call.file = Some(s(e, "file").to_string());
                }
            }
            event
        }
    };
    if let Some(title) = given(s(e, "session_title")) {
        event.title = Some(clip(&title, 80));
    }
    Some(event)
}
