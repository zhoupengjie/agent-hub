// OpenCode 2's events, as a plugin inside it sends them ({ type, sessionID,
// ... }, with what only it knows put in: the folder, the session's title,
// the person's words, a call's tool, the turn's last text), by the names
// Claude Code's hooks have.
use serde_json::{json, Map, Value};

use crate::s;

// A tool as Claude Code names it, with its input as Claude Code's has it.
pub fn as_claude(tool: &str, input: &Value) -> (String, Value) {
    let i = |key: &str| json!(s(input, key));
    let (name, input) = match tool {
        "shell" => ("Bash", json!({ "command": i("command"), "description": i("description") })),
        "read" => ("Read", json!({ "file_path": i("path") })),
        "edit" => ("Edit", json!({ "file_path": i("path") })),
        "write" => ("Write", json!({ "file_path": i("path") })),
        "patch" => ("apply_patch", json!({ "command": i("patchText") })),
        "glob" => ("Glob", json!({ "pattern": i("pattern") })),
        "grep" => ("Grep", json!({ "pattern": i("pattern") })),
        "webfetch" => ("WebFetch", json!({ "url": i("url") })),
        "websearch" => ("WebSearch", json!({ "query": i("query") })),
        "subagent" => ("Agent", json!({ "description": i("description"), "subagent_type": i("agent") })),
        "skill" => ("Skill", json!({ "skill": i("id") })),
        _ => return (tool.to_string(), input.clone()),
    };
    (name.to_string(), input)
}

// OpenCode's event under the name Claude Code's hook has for it, with the
// fields its hooks have (the plugin's kept too), or Null for one that is
// nothing to a session: as OpenCode 2.0.29 sends them (2026-10-15).
pub fn normalize(e: &Value) -> Value {
    let error = e.get("error").unwrap_or(&Value::Null);
    let name = match s(e, "type") {
        "session.created" => "SessionStart",
        "session.execution.started" => "UserPromptSubmit",
        "session.tool.called" => "PreToolUse",
        "session.tool.success" => "PostToolUse",
        // Turned down by the person ("The user declined this tool call",
        // aborted, as 2.0.29 says it), or stopped: no failure of the tool's.
        "session.tool.failed" if matches!(s(error, "type"), "aborted" | "permission.rejected") => "PostToolUse",
        "session.tool.failed" => "PostToolUseFailure",
        "permission.asked" => "PermissionRequest",
        "permission.replied" => "PermissionReplied",
        "form.created" => "Question",
        "form.replied" | "form.cancelled" => "Answered",
        "session.execution.succeeded" => "Stop",
        // A turn ended by a permission turned down, or cut short, is stopped, not failed.
        "session.execution.failed" if matches!(s(error, "type"), "aborted" | "permission.rejected") => "Interrupt",
        "session.execution.failed" => "StopFailure",
        // Another turn took its place: that one goes on.
        "session.execution.interrupted" if s(e, "reason") == "superseded" => return Value::Null,
        "session.execution.interrupted" => "Interrupt",
        "session.deleted" => "SessionEnd",
        _ => return Value::Null,
    };
    let mut out: Map<String, Value> = e.as_object().cloned().unwrap_or_default();
    out.insert("hook_event_name".into(), json!(name));
    out.insert("session_id".into(), json!(s(e, "sessionID")));
    out.insert("cwd".into(), json!(s(e, "directory")));
    out.insert("agent_id".into(), json!(s(e, "agentID")));
    if !s(e, "title").is_empty() {
        out.insert("session_title".into(), json!(s(e, "title")));
    }
    let input = e.get("input").cloned().unwrap_or_else(|| json!({}));
    match name {
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" => {
            let (tool, input) = as_claude(s(e, "tool"), &input);
            out.insert("tool_name".into(), json!(tool));
            out.insert("tool_input".into(), input);
            out.insert("tool_use_id".into(), json!(s(e, "id")));
        }
        // The permission asked, by OpenCode's own name for it (shell, edit,
        // external_directory, …), with the call's input as Claude Code's has it.
        "PermissionRequest" => {
            let action = if s(e, "action").is_empty() { s(e, "tool") } else { s(e, "action") };
            let (_, input) = as_claude(s(e, "tool"), &input);
            out.insert("tool_name".into(), json!(action));
            out.insert("tool_use_id".into(), json!(s(e.get("source").unwrap_or(&Value::Null), "id")));
            out.insert("permission_id".into(), json!(s(e, "id")));
            out.insert("tool_input".into(), input);
        }
        "PermissionReplied" => {
            out.insert("permission_id".into(), json!(s(e, "requestID")));
        }
        "Stop" => {
            out.insert("last_assistant_message".into(), json!(s(e, "text")));
        }
        _ => {}
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, more: Value) -> Value {
        let mut e = json!({ "type": kind, "sessionID": "ses_1", "directory": "Z:\\nowhere\\oc3", "title": "Shell echo command" });
        for (k, v) in more.as_object().unwrap() {
            e[k] = v.clone();
        }
        e
    }

    #[test]
    fn opencode_s_events_by_claude_code_s_names() {
        let called = normalize(&event("session.tool.called", json!({ "id": "call_1", "tool": "shell", "input": { "command": "echo hi" } })));
        assert_eq!(
            (called["hook_event_name"].as_str(), called["session_id"].as_str(), called["cwd"].as_str(), called["session_title"].as_str()),
            (Some("PreToolUse"), Some("ses_1"), Some("Z:\\nowhere\\oc3"), Some("Shell echo command"))
        );
        assert_eq!((called["tool_name"].as_str(), &called["tool_input"], called["tool_use_id"].as_str()), (Some("Bash"), &json!({ "command": "echo hi", "description": "" }), Some("call_1")));
        let asked = normalize(&event("permission.asked", json!({ "id": "per_1", "action": "shell", "source": { "type": "tool", "id": "call_1" }, "tool": "shell", "input": { "command": "echo hi" } })));
        assert_eq!(
            (asked["hook_event_name"].as_str(), asked["tool_name"].as_str(), asked["permission_id"].as_str(), asked["tool_use_id"].as_str()),
            (Some("PermissionRequest"), Some("shell"), Some("per_1"), Some("call_1"))
        );
        let replied = normalize(&event("permission.replied", json!({ "requestID": "per_1", "reply": "once" })));
        assert_eq!((replied["hook_event_name"].as_str(), replied["permission_id"].as_str()), (Some("PermissionReplied"), Some("per_1")));
        assert_eq!(normalize(&event("session.execution.succeeded", json!({ "text": "Done." })))["last_assistant_message"], "Done.");
        assert_eq!(normalize(&event("session.tool.failed", json!({ "error": { "type": "aborted" } })))["hook_event_name"], "PostToolUse");
        assert_eq!(normalize(&event("session.execution.failed", json!({ "error": { "type": "permission.rejected" } })))["hook_event_name"], "Interrupt");
        for kind in ["session.renamed", "session.step.started", "message.updated", ""] {
            assert!(normalize(&event(kind, json!({}))).is_null(), "{kind}");
        }
        assert!(normalize(&event("session.execution.interrupted", json!({ "reason": "superseded" }))).is_null());
        assert_eq!(as_claude("patch", &json!({ "patchText": "x" })), ("apply_patch".to_string(), json!({ "command": "x" })));
        assert_eq!(as_claude("github_create_issue", &json!({ "a": 1 })), ("github_create_issue".to_string(), json!({ "a": 1 })));
    }
}
