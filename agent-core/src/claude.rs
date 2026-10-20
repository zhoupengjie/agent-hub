// Claude Code's hook events as a session's doing (to_event), and its own
// files (the session's name in its transcript). Its words are the ones the
// other agents' are read in too: ZCode's come so, and omp's, Reasonix's and
// DeepSeek Harness's are put so first.
//
// What there is: session_id, cwd, hook_event_name, transcript_path and, as
// each has them, source, prompt, tool_name, tool_input, tool_use_id,
// is_interrupt, mcp_server_name, task_subject, last_assistant_message,
// error and error_details, session_title.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde_json::Value;

use crate::event::{given, Agent, AgentEvent, AnswerIn, Call, Failure, Kind, Todo, TodoItem, Wait, What};
use crate::s;
use crate::text::clip;

// The tools that change a file.
const EDITS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

// The kinds of error Claude Code says a turn failed with (StopFailure's error).
const ERROR_KINDS: [&str; 10] = [
    "authentication_failed",
    "oauth_org_not_allowed",
    "account_on_hold",
    "billing_error",
    "rate_limit",
    "overloaded",
    "invalid_request",
    "model_not_found",
    "server_error",
    "max_output_tokens",
];

// What a tool that stops for the person waits on.
pub fn asks_person(tool: &str) -> Option<Wait> {
    match tool {
        "AskUserQuestion" => Some(Wait::Question),
        "ExitPlanMode" => Some(Wait::Plan),
        _ => None,
    }
}

// A change to the session's to-do list, from the tools that keep it.
pub fn todo_of(tool: &str, input: &Value) -> Option<Todo> {
    let i = |key: &str| s(input, key).to_string();
    match tool {
        "TodoWrite" => {
            let items = input
                .get("todos")?
                .as_array()?
                .iter()
                .map(|t| TodoItem { text: s(t, "content").to_string(), active: s(t, "activeForm").to_string(), status: s(t, "status").to_string() })
                .collect();
            Some(Todo::Set { items })
        }
        "TaskCreate" => Some(Todo::Add { item: TodoItem { text: i("subject"), active: i("activeForm"), status: String::new() } }),
        "TaskUpdate" => {
            let id = match input.get("taskId") {
                Some(Value::String(id)) => id.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => return None,
            };
            Some(Todo::Update { id, status: given(s(input, "status")), text: given(s(input, "subject")), active: given(s(input, "activeForm")) })
        }
        _ => None,
    }
}

// Why a turn failed: one of Claude Code's kinds, or its own words.
pub fn failure_of(e: &Value) -> Failure {
    let kind = s(e, "error");
    if ERROR_KINDS.contains(&kind) {
        Failure::Kind { kind: kind.to_string() }
    } else {
        Failure::Said { said: s(e, "error_details").to_string() }
    }
}

// A Claude Code hook event (or another agent's, `agent`, in its words) as
// a session's doing, or None for one that changes nothing.
pub fn to_event(e: &Value, agent: Agent) -> Option<AgentEvent> {
    if !e.is_object() {
        return None;
    }
    let (name, tool) = (s(e, "hook_event_name"), s(e, "tool_name"));
    let input = e.get("tool_input").cloned().unwrap_or(Value::Null);
    let call = || Call::new(s(e, "tool_use_id"), tool, input.clone());
    let kind = match name {
        // A compaction starts a new context mid-work: nothing changed.
        "SessionStart" if s(e, "source") == "compact" => return None,
        "SessionStart" => Kind::SessionStart,
        "SessionEnd" => Kind::SessionEnd,
        // Only the person's words name the work, not a turn the system began.
        "UserPromptSubmit" => Kind::TurnStart { prompt: given(s(e, "prompt")).filter(|_| matches!(s(e, "source"), "" | "user")) },
        "PreToolUse" => match asks_person(tool) {
            Some(wait) => Kind::Waiting { wait },
            None => Kind::ToolStart { call: Call { todo: todo_of(tool, &input), ..call() } },
        },
        "PostToolUse" => {
            let path = if tool == "NotebookEdit" { s(&input, "notebook_path") } else { s(&input, "file_path") };
            Kind::ToolDone { call: Call { file: given(path).filter(|_| EDITS.contains(&tool)), ..call() } }
        }
        // An interrupt is the person stopping it, not the tool failing.
        "PostToolUseFailure" if e.get("is_interrupt").and_then(Value::as_bool) == Some(true) => return None,
        "PostToolUseFailure" => Kind::ToolFailed { call: call(), quiet: false },
        "PermissionRequest" => Kind::Waiting { wait: asks_person(tool).unwrap_or(Wait::Approval { what: What::Call { call: call() }, answer_in: AnswerIn::Here }) },
        "Elicitation" => Kind::Waiting { wait: Wait::Input { server: s(e, "mcp_server_name").to_string() } },
        "TaskCompleted" => Kind::TaskDone { subject: given(s(e, "task_subject")) },
        "Stop" => Kind::Done,
        "StopFailure" => Kind::Failed { error: failure_of(e) },
        _ => return None,
    };
    // The session's name: given with the event, or else, at the moments it
    // may have changed (a turn begins or ends), read from the transcript.
    let title = match s(e, "session_title") {
        "" if matches!(name, "UserPromptSubmit" | "Stop" | "StopFailure" | "SessionStart") => transcript_title(s(e, "transcript_path")),
        "" => None,
        given => Some(clip(given, 80)),
    };
    Some(AgentEvent {
        agent,
        session: s(e, "session_id").to_string(),
        within: false,
        cwd: given(s(e, "cwd")),
        turn: None,
        title,
        reply: given(s(e, "last_assistant_message")),
        kind,
    })
}

// An event of the agent's own, in none of Claude Code's words: what it
// says of a session, and no more.
pub(crate) fn bare(agent: Agent, session: &str, cwd: &str, kind: Kind) -> AgentEvent {
    AgentEvent { agent, session: session.to_string(), within: false, cwd: given(cwd), turn: None, title: None, reply: None, kind }
}

// How much of a transcript's end is read for its title: Claude Code writes
// the title again near the end as it goes.
const TITLE_TAIL: u64 = 256 * 1024;

// Claude Code's name for the session from its transcript: the last name the
// person (or the app) gave it, else the last one Claude made up.
pub fn transcript_title(path: &str) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TITLE_TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let (mut custom, mut ai) = (None, None);
    for line in text.lines().rev() {
        if custom.is_some() {
            break;
        }
        let is_custom = line.contains("\"custom-title\"");
        if !is_custom && (ai.is_some() || !line.contains("\"ai-title\"")) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        match s(&entry, "type") {
            "custom-title" => custom = Some(s(&entry, "customTitle").to_string()),
            "ai-title" => ai = Some(s(&entry, "aiTitle").to_string()),
            _ => {}
        }
    }
    custom.or(ai).map(|t| clip(&t, 80)).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_s_name_from_its_transcript() {
        let dir = std::env::temp_dir().join(format!("agent-core-claude-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("t.jsonl");
        let lines = [
            r#"{"type":"ai-title","aiTitle":"Made up","sessionId":"s"}"#,
            r#"{"type":"custom-title","customTitle":"Given","sessionId":"s"}"#,
            r#"{"type":"user","message":"hi"}"#,
            r#"{"type":"ai-title","aiTitle":"Made up later","sessionId":"s"}"#,
        ];
        std::fs::write(&transcript, lines.join("\n")).unwrap();
        assert_eq!(transcript_title(&transcript.to_string_lossy()).as_deref(), Some("Given"));
        std::fs::write(&transcript, [lines[0], lines[3]].join("\n")).unwrap();
        assert_eq!(transcript_title(&transcript.to_string_lossy()).as_deref(), Some("Made up later"));
        assert_eq!(transcript_title("Z:\\no\\such\\file.jsonl"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
