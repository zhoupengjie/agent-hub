// Codex hook events as a session's doing, with the turn they belong to.
//
// Codex runs its hooks as commands (an app's own program, which hands each
// event to the app's port). What differs from Claude Code: the shell tool is
// Bash and every edit is apply_patch (the patch is tool_input.command,
// which OpenCode's patch tool takes too: patched); a failed tool call gets
// no PostToolUse, a failed request no Stop; Interrupt is the person stopping
// a turn. Each step's hooks may run in the background, so one can arrive
// after its turn is over: the turn id tells it apart.
use serde_json::Value;

use crate::event::{given, Agent, AgentEvent, AnswerIn, Call, Kind, Wait, What};
use crate::s;
use crate::text::absolute;

// The files a patch adds, changes or deletes.
pub fn patched(patch: &str) -> Vec<&str> {
    patch
        .lines()
        .filter_map(|line| ["*** Add File: ", "*** Update File: ", "*** Delete File: "].iter().find_map(|p| line.strip_prefix(p)))
        .map(str::trim)
        .collect()
}

// A Codex hook event as a session's doing, or None for one that changes nothing.
pub fn to_event(e: &Value) -> Option<AgentEvent> {
    if !e.is_object() {
        return None;
    }
    let (name, tool) = (s(e, "hook_event_name"), s(e, "tool_name"));
    let input = e.get("tool_input").cloned().unwrap_or(Value::Null);
    let call = || Call::new(s(e, "tool_use_id"), tool, input.clone());
    let kind = match name {
        "SessionStart" if s(e, "source") == "compact" => return None,
        "SessionStart" => Kind::SessionStart,
        "SessionEnd" => Kind::SessionEnd,
        "UserPromptSubmit" => Kind::TurnStart { prompt: given(s(e, "prompt")) },
        // Its question to the person waits on them like a prompt.
        "PreToolUse" if tool == "request_user_input" => Kind::Waiting { wait: Wait::question() },
        "PreToolUse" => Kind::ToolStart { call: call() },
        // Every file its patch changed, by its whole path.
        "PostToolUse" => {
            let files: Vec<String> = if tool == "apply_patch" { patched(s(&input, "command")).iter().map(|f| absolute(s(e, "cwd"), f)).collect() } else { Vec::new() };
            Kind::ToolDone { call: Call { file: files.first().cloned(), files, ..call() } }
        }
        "PermissionRequest" => Kind::Waiting { wait: Wait::Approval { what: What::Call { call: call() }, answer_in: AnswerIn::Here } },
        "Stop" => Kind::Done,
        // Stopped by the person.
        "Interrupt" => Kind::Stopped,
        _ => return None,
    };
    Some(AgentEvent {
        agent: Agent::Codex,
        session: s(e, "session_id").to_string(),
        within: false,
        cwd: given(s(e, "cwd")),
        turn: given(s(e, "turn_id")),
        title: None,
        reply: given(s(e, "last_assistant_message")),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_a_patch_touches() {
        let patch = "*** Begin Patch\n*** Add File: hello.txt\n+hi\n*** Update File: D:\\w\\b.txt\n@@\n-a\n+b\n*** Delete File: old.txt\n*** End Patch";
        assert_eq!(patched(patch), ["hello.txt", "D:\\w\\b.txt", "old.txt"]);
        assert!(patched("not a patch").is_empty());
        // A patch done: every file it changed, whole (zhoupengjie/agent-hub#2).
        let done = serde_json::json!({ "hook_event_name": "PostToolUse", "session_id": "s", "cwd": "D:\\w", "tool_name": "apply_patch", "tool_input": { "command": patch } });
        let Some(AgentEvent { kind: Kind::ToolDone { call }, .. }) = to_event(&done) else { panic!("no tool done") };
        assert_eq!((call.file.as_deref(), call.files.as_slice()), (Some("D:\\w\\hello.txt"), &["D:\\w\\hello.txt".to_string(), "D:\\w\\b.txt".into(), "D:\\w\\old.txt".into()][..]));
    }
}
