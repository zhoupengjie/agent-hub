// Antigravity (agy, and its 2.0 app): what its hooks' payloads lack, from
// its transcript.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use serde_json::{json, Value};

use crate::s;

// So much of the transcript's end is read: a turn's lines are there.
const TAIL: u64 = 256 * 1024;

// The person's words for a turn starting (prompt) and the step they are
// (userStep: the turn), and the last reply for one ending
// (last_assistant_message), from the transcript's last lines, as agy 1.3.4
// writes it: one step a line, the person's a USER_INPUT whose content holds
// them in <USER_REQUEST>, the model's answer a PLANNER_RESPONSE with content.
// Undocumented: nothing when it reads otherwise.
pub fn with_transcript(e: &mut Value) {
    let event = s(e, "hook_event_name");
    let wants = match event {
        "PreInvocation" if e.get("invocationNum").and_then(Value::as_u64) == Some(0) => "USER_INPUT",
        "Stop" => "PLANNER_RESPONSE",
        _ => return,
    };
    let Some(lines) = tail(s(e, "transcriptPath")) else { return };
    let mut steps = lines.lines().rev().filter_map(|line| serde_json::from_str::<Value>(line).ok());
    if wants == "USER_INPUT" {
        // The person's last words, with or without any (an image alone).
        let Some(step) = steps.find(|step| s(step, "type") == wants) else { return };
        if let Some(at) = step.get("step_index").and_then(Value::as_u64) {
            e["userStep"] = json!(at);
        }
        let content = s(&step, "content");
        let said = content.split_once("<USER_REQUEST>").and_then(|(_, rest)| rest.split_once("</USER_REQUEST>")).map_or(content, |(said, _)| said);
        if !said.trim().is_empty() {
            e["prompt"] = json!(said.trim());
        }
    } else if let Some(step) = steps.find(|step| s(step, "type") == wants && !s(step, "content").trim().is_empty()) {
        e["last_assistant_message"] = json!(s(&step, "content").trim());
    }
}

fn tail(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}
