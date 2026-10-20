// Reasonix hook events as a session's doing: read as Claude Code's
// (claude.rs).
//
// Reasonix runs the hooks in its global settings.json, each as a command (an
// app's own program with the event's name, which hands it to the app's
// port). Its payload is its own (its hook package's Payload, 2.x and the Go
// 1.x alike): event, sessionId, cwd (the workspace's root), toolName and
// toolArgs (its tools' own names and arguments: write_file with path),
// toolResult, prompt and turn, lastAssistantText, error, isInterrupt,
// source, reason, message and notificationType; read as Claude Code's
// fields and tools (the table Reasonix itself uses for the Claude Code hooks
// it runs: as_claude). What differs, as its source has it (studio, 2.43,
// 2026-10-17):
//   - a failed turn fires StopFailure, then its settings' Stop hooks again
//     with the same payload (error set): that Stop is no ending of its own;
//     a failed tool, PostToolUseFailure and then PostToolUse the same way;
//     the person stopping a turn is a StopFailure with isInterrupt;
//   - SessionStart comes with the session's first turn, not as it opens;
//   - its subagents (a task, explore, ...), its planner and its guardian
//     fire the tool hooks under ids of their own, the session's with
//     ":subagent:<ref>", ":planner" or ":guardian" after it: told as the
//     session's steps while a turn of it is under way (within), never as a
//     session of their own;
//   - a PermissionRequest comes before its dialog, a Notification
//     (permission_prompt) as the dialog shows: the first says what waits,
//     the second only says it again (but for a plan's approval and Auto
//     Guard's, which have no PermissionRequest); its questions (the ask
//     tool) come as PreToolUse, then a Notification (question_prompt).
// Its approvals are answered in Reasonix (its hook hands one on and is gone).
use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::claude::{bare, to_event as claude_event};
use crate::event::{Agent, AgentEvent, AnswerIn, Kind, Wait, What};
use crate::s;

// When each session last asked for an approval through PermissionRequest
// (ms), so that its Notification for the same dialog, just after, changes
// nothing: the first already said what waits.
static ASKED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
// How long after a PermissionRequest its Notification is taken as the same
// dialog's (it comes at once, in its own goroutine).
pub const SAME_ASK_MS: u64 = 30_000;

fn asked(session: &str, now: u64) {
    let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
    let asked = asked.get_or_insert_with(HashMap::new);
    asked.retain(|_, at| now.saturating_sub(*at) < SAME_ASK_MS);
    asked.insert(session.to_string(), now);
}

fn was_asked(session: &str, now: u64) -> bool {
    let asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
    asked.as_ref().and_then(|a| a.get(session)).is_some_and(|at| now.saturating_sub(*at) < SAME_ASK_MS)
}

// A Reasonix event at `now` (which tells a Notification from the
// PermissionRequest just before it) as a session's doing, or None for one
// that changes nothing.
pub fn to_event_at(e: &Value, now: u64) -> Option<AgentEvent> {
    if !e.is_object() {
        return None;
    }
    let (session, is_within) = whose(s(e, "sessionId"));
    let failed = !s(e, "error").is_empty();
    let is_interrupt = e.get("isInterrupt").and_then(Value::as_bool) == Some(true);
    let claude = as_claude(e);
    let own = |kind| bare(Agent::Reasonix, session, s(e, "cwd"), kind);
    let mut event = match s(e, "event") {
        // A prompt for its approval, answered in Reasonix.
        "PermissionRequest" => {
            let mut event = claude_event(&claude, Agent::Reasonix)?;
            if let Kind::Waiting { wait: Wait::Approval { answer_in, .. } } = &mut event.kind {
                *answer_in = AnswerIn::App { name: "Reasonix".into() };
            }
            asked(session, now);
            event
        }
        "Notification" => own(Kind::Waiting { wait: notification(e, session, now)? }),
        // A failed turn's or tool's, again as its plain end: told already.
        "Stop" | "PostToolUse" if failed => return None,
        // The person stopped the turn.
        "StopFailure" if is_interrupt => own(Kind::Stopped),
        // Its subagents' bounds and model calls, compactions: nothing.
        "SubagentStart" | "SubagentStop" | "PostLLMCall" | "PreCompact" => return None,
        _ => claude_event(&claude, Agent::Reasonix)?,
    };
    event.session = session.to_string();
    if is_within {
        // A subagent's step, as its session's; never a session, a turn or an
        // ending of its own, nor a reaction or words of its own.
        if !matches!(event.kind, Kind::ToolStart { .. } | Kind::ToolDone { .. } | Kind::ToolFailed { .. } | Kind::Waiting { .. } | Kind::Working) {
            return None;
        }
        event.within = true;
        event.reply = None;
        if let Kind::ToolFailed { quiet, .. } = &mut event.kind {
            *quiet = true;
        }
    }
    Some(event)
}

// What it wants the person's attention for: an approval (said already by its
// PermissionRequest, unless none came: a plan's, Auto Guard's), a question
// (said already by the ask tool's start, and said the same again), or
// something else, in its own words.
fn notification(e: &Value, session: &str, now: u64) -> Option<Wait> {
    let said = s(e, "message").trim();
    match s(e, "notificationType") {
        "permission_prompt" if was_asked(session, now) => None,
        "permission_prompt" => Some(Wait::Approval { what: What::Said { said: said.to_string() }, answer_in: AnswerIn::App { name: "Reasonix".into() } }),
        "question_prompt" => Some(Wait::Question),
        _ if said.is_empty() => Some(Wait::Notice { said: None }),
        _ => Some(Wait::Notice { said: Some(said.to_string()) }),
    }
}

// Its tools as Claude Code names them (Reasonix's own table for the hooks it
// imports from Claude Code: claudeToolNames in its hook package), each
// subagent's starter as Agent; one of its own (code_index, move_file, ...)
// keeps its name, as does an MCP tool (mcp__<server>__<tool> in both).
fn claude_name(tool: &str) -> &str {
    match tool {
        "bash" => "Bash",
        "read_file" => "Read",
        "write_file" => "Write",
        "edit_file" => "Edit",
        "multi_edit" => "MultiEdit",
        "glob" => "Glob",
        "grep" => "Grep",
        "web_fetch" => "WebFetch",
        "web_search" => "WebSearch",
        "ask" => "AskUserQuestion",
        "run_skill" | "read_only_skill" => "Skill",
        "todo_write" => "TodoWrite",
        "notebook_edit" => "NotebookEdit",
        "bash_output" | "wait" => "TaskOutput",
        "kill_shell" => "TaskStop",
        "task" | "read_only_task" | "parallel_tasks" | "explore" | "locate" | "research" | "review" | "security_review" => "Agent",
        _ => tool,
    }
}

// Its tool's arguments with Claude Code's names for them (the same table's
// claudeToolInputKeyRenames, for what is read of them: a file's path, a
// skill's name, a subagent's task); a file's path made whole from the
// session's folder, as its tools resolve one (the files a turn changed).
fn claude_input(tool: &str, args: &Value, cwd: &str) -> Value {
    let renames: &[(&str, &str)] = match tool {
        "read_file" | "write_file" | "edit_file" | "multi_edit" => &[("path", "file_path")],
        "notebook_edit" => &[("path", "notebook_path")],
        "run_skill" | "read_only_skill" => &[("name", "skill")],
        "explore" | "locate" | "research" | "review" | "security_review" => &[("task", "prompt")],
        _ => &[],
    };
    let Some(fields) = args.as_object() else { return args.clone() };
    let mut out: Map<String, Value> = fields.clone();
    for (from, to) in renames {
        if let Some(v) = out.remove(*from) {
            out.insert((*to).into(), v);
        }
    }
    for key in ["file_path", "notebook_path"] {
        let path = out.get(key).and_then(Value::as_str).unwrap_or("");
        if !path.is_empty() && !cwd.is_empty() && !std::path::Path::new(path).is_absolute() {
            let whole = std::path::Path::new(cwd).join(path).to_string_lossy().into_owned();
            out.insert(key.into(), json!(whole));
        }
    }
    Value::Object(out)
}

// The person's words in the prompt Reasonix hands its UserPromptSubmit hook:
// the input it composed for the model, which begins with blocks of its own
// (<current-date>…</current-date>, <available-skills>…</available-skills>,
// as 2.44 sends it, 2026-10-18) before them, each closed on a line of its
// own; those taken off. Words that begin with a tag of their own (closed on
// the same line) are left as they are.
pub fn person_words(prompt: &str) -> &str {
    let mut rest = prompt.trim_start();
    while let Some(after) = rest.strip_prefix('<') {
        let Some(name) = after.split_once('>').map(|(n, _)| n).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) else { break };
        let close = format!("\n</{name}>");
        let Some(end) = rest.find(&close) else { break };
        rest = rest[end + close.len()..].trim_start();
    }
    if rest.is_empty() { prompt } else { rest }
}

// The event in Claude Code's words: Reasonix's own payload (camelCase) with
// the fields renamed, its tool and arguments as Claude Code's.
pub fn as_claude(e: &Value) -> Value {
    let tool = s(e, "toolName");
    let args = match e.get("toolArgs") {
        // Its arguments as the model gave them: JSON, or a string of it.
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    };
    let mut out = json!({
        "hook_event_name": s(e, "event"),
        "session_id": s(e, "sessionId"),
        "cwd": s(e, "cwd"),
        "tool_name": claude_name(tool),
        "tool_input": claude_input(tool, &args, s(e, "cwd")),
        "prompt": person_words(s(e, "prompt")),
        "last_assistant_message": s(e, "lastAssistantText"),
        "source": s(e, "source"),
        "reason": s(e, "reason"),
        "message": s(e, "message"),
        "notification_type": s(e, "notificationType"),
        "is_interrupt": e.get("isInterrupt").and_then(Value::as_bool).unwrap_or(false),
        // Why a turn failed, in its own words.
        "error_details": s(e, "error"),
    });
    if !s(e, "callId").is_empty() {
        out["tool_use_id"] = json!(s(e, "callId"));
    }
    out
}

// The session an id is, and whether it is one working for it (a subagent,
// its planner, its guardian: Reasonix's hook runner names theirs after the
// session's, ForRole): "<session>:subagent:<ref>", "<session>:planner",
// "<session>:guardian".
pub fn whose(id: &str) -> (&str, bool) {
    if let Some((session, _)) = id.split_once(":subagent:") {
        return (session, true);
    }
    for role in [":planner", ":guardian"] {
        if let Some(session) = id.strip_suffix(role) {
            return (session, true);
        }
    }
    (id, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_person_s_words_past_reasonix_s_own_blocks() {
        // As Reasonix 2.44 composes it (trimmed): its blocks, then the person's words.
        let prompt = "<current-date>\nToday's date is 2026-10-18 (Sunday); the local time zone is UTC+02:00.\n</current-date>\n\n<available-skills>\n# Skills\n- explore [🧬 subagent] — Explore…\n</available-skills>\n\nList the files in this folder and reply in one short sentence.";
        assert_eq!(person_words(prompt), "List the files in this folder and reply in one short sentence.");
        // Plain words, words beginning with a tag of their own, a block never closed: as they are.
        assert_eq!(person_words("fix the login page"), "fix the login page");
        assert_eq!(person_words("<b>bold</b> is a tag"), "<b>bold</b> is a tag");
        assert_eq!(person_words("<div class=x>keep</div>"), "<div class=x>keep</div>");
        assert_eq!(person_words("<current-date>\nno end"), "<current-date>\nno end");
        assert_eq!(person_words("<current-date>x</current-date>"), "<current-date>x</current-date>");
    }

    #[test]
    fn whose_a_session_is() {
        assert_eq!(whose("20261018-101500.123456789-deepseek-chat"), ("20261018-101500.123456789-deepseek-chat", false));
        assert_eq!(whose("s1:subagent:call_7"), ("s1", true));
        assert_eq!(whose("s1:subagent:a:subagent:b"), ("s1", true));
        assert_eq!(whose("s1:planner"), ("s1", true));
        assert_eq!(whose("s1:guardian"), ("s1", true));
    }

    #[test]
    fn reasonix_s_events_in_claude_code_s_words() {
        let e = json!({ "event": "PostToolUse", "sessionId": "rx", "cwd": "Z:\\nowhere\\rx", "toolName": "write_file", "toolArgs": "{\"path\":\"note.txt\"}", "callId": "c1" });
        let c = as_claude(&e);
        let whole = std::path::Path::new("Z:\\nowhere\\rx").join("note.txt").to_string_lossy().into_owned();
        assert_eq!((c["hook_event_name"].as_str(), c["tool_name"].as_str(), c["tool_input"]["file_path"].as_str(), c["tool_use_id"].as_str()), (Some("PostToolUse"), Some("Write"), Some(whole.as_str()), Some("c1")));
        assert_eq!(as_claude(&json!({ "toolName": "explore", "toolArgs": { "task": "Find TODOs" } }))["tool_input"], json!({ "prompt": "Find TODOs" }));
        assert_eq!(as_claude(&json!({ "toolName": "code_index" }))["tool_name"], "code_index");
    }
}
