// Antigravity (agy, and its 2.0 app) hook events as a session's doing, with
// what its payloads lack read from its transcript.
//
// Antigravity runs its hooks as commands (an app's own program with the
// event's name, which its payload does not carry). What there is, as agy
// 1.3.4 sends it (2026-10-13): camelCase; conversationId the session,
// workspacePaths the folders it works in, transcriptPath its transcript.
// SessionStart; PreInvocation before each call of the model, the first of a
// turn invocationNum 0; PostToolUse with toolCall { name, args }, the args
// saying what was done in words of their own (toolAction: "Creating
// hello.txt"); Stop with fullyIdle, terminationReason and error. No prompt
// waiting on the person, no tool starting (PreToolUse would have to answer
// for every tool, and an answer missing denies it): the work and its end.
// The person's words and the last reply come from the transcript
// (with_transcript), as the payload has neither.
//
// A command it leaves running in the background (run_command's
// WaitMsBeforeAsync), as agy 1.3.4 does it (2026-10-14): the model answers,
// Stop with fullyIdle false (still at work); the command ends, its
// PostToolUse comes, a SYSTEM_MESSAGE step says so and the model is called
// again from invocationNum 0, then Stop, fully idle. That second start is the
// same turn: a turn is known by the person's USER_INPUT step it answers
// (turn), else it counts twice and its time from the second start.
use serde_json::{json, Value};

use crate::event::{given, Agent, AgentEvent, Call, Failure, Kind};
use crate::s;
use crate::text::tail;

// An Antigravity hook event (its name put in by the program that hands it
// on) as a session's doing, given the person's words and the last reply
// from its transcript first; None for one that changes nothing.
pub fn to_event(e: &mut Value) -> Option<AgentEvent> {
    with_transcript(e);
    if !e.is_object() {
        return None;
    }
    let e = &*e;
    let call_of = e.get("toolCall").unwrap_or(&Value::Null);
    let (tool, args) = (s(call_of, "name"), call_of.get("args").cloned().unwrap_or(Value::Null));
    // Its own words for what a tool did.
    let words = [s(&args, "toolAction"), s(&args, "toolSummary")].into_iter().map(str::trim).find(|w| !w.is_empty()).map(str::to_string);
    let call = || Call { intent: words.clone(), ..Call::new("", tool, args.clone()) };
    let mut turn = None;
    let mut reply = None;
    let error = s(e, "error");
    let kind = match s(e, "hook_event_name") {
        "SessionStart" => Kind::SessionStart,
        // The turn's first call of the model: a turn begins.
        "PreInvocation" if e.get("invocationNum").and_then(Value::as_u64) == Some(0) => {
            turn = e.get("userStep").and_then(Value::as_u64).map(|step| step.to_string());
            Kind::TurnStart { prompt: given(s(e, "prompt")) }
        }
        "PreInvocation" => Kind::Working,
        "PostToolUse" if !error.is_empty() => Kind::ToolFailed { call: call(), quiet: false },
        // A file it wrote (write_to_file's, the edits' TargetFile): what the
        // turn changed. (A file read is view_file's AbsolutePath.)
        "PostToolUse" => Kind::ToolDone { call: Call { file: given(s(&args, "TargetFile")), ..call() } },
        "Stop" if !error.is_empty() => Kind::Failed { error: Failure::Said { said: error.to_string() } },
        // Stopped by the person.
        "Stop" if ["CANCEL", "INTERRUPT", "ABORT"].iter().any(|w| s(e, "terminationReason").to_uppercase().contains(w)) => Kind::Stopped,
        // Not done yet (something of it still runs).
        "Stop" if e.get("fullyIdle").and_then(Value::as_bool) == Some(false) => return None,
        "Stop" => {
            reply = given(s(e, "last_assistant_message"));
            Kind::Done
        }
        _ => return None,
    };
    let folder = e.get("workspacePaths").and_then(Value::as_array).and_then(|w| w.first()).and_then(Value::as_str).unwrap_or("");
    Some(AgentEvent { agent: Agent::Agy, session: s(e, "conversationId").to_string(), within: false, cwd: given(folder), turn, title: None, reply, kind })
}

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
    let Some(lines) = tail(s(e, "transcriptPath"), TAIL) else { return };
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
