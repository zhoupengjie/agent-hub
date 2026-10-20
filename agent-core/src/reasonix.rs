// Reasonix's hook events (its hook package's Payload, 2.x and the Go 1.x
// alike: event, sessionId, cwd, toolName and toolArgs, toolResult, prompt
// and turn, lastAssistantText, error, isInterrupt, source, reason, message
// and notificationType) read as Claude Code's, by the table Reasonix itself
// uses for the Claude Code hooks it runs; and whose a session is.
use serde_json::{json, Map, Value};

use crate::s;

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
