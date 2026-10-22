// Codex hook events as a session's doing, with the turn they belong to.
//
// Codex runs its hooks as commands (an app's own program, which hands each
// event to the app's port). What differs from Claude Code: the shell tool is
// Bash and every edit is apply_patch (the patch is tool_input.command,
// which OpenCode's patch tool takes too: patched); a failed tool call gets
// no PostToolUse, a failed request no Stop; Interrupt is the person stopping
// a turn. Each step's hooks may run in the background, so one can arrive
// after its turn is over: the turn id tells it apart. Its desktop app asks
// a model things of its own accord too (a suggested next message for the
// person), as turns with prompts of their own: no doing of the person's.
use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::Value;

use crate::event::{given, Agent, AgentEvent, AnswerIn, Call, Kind, Wait, What};
use crate::s;
use crate::text::absolute;

// What the desktop app's own requests begin with ("This is an app-generated
// request for a suggested next user message…", zhoupengjie/Wakuwaku#74).
const APP_GENERATED: &str = "This is an app-generated request";

// The headings the desktop app puts the person's words under, after the
// files they mentioned or pasted, the comments and the text they picked.
const MY_REQUEST: [&str; 2] = ["## My request:", "## My request for Codex:"];

// The person's words in a prompt Codex hands its UserPromptSubmit hook:
// what follows its last "## My request:" heading, as the desktop app reads
// it; the whole prompt where there is none.
pub fn person_words(prompt: &str) -> &str {
    match MY_REQUEST.iter().filter_map(|h| prompt.rfind(h).map(|at| at + h.len())).max() {
        Some(at) => prompt[at..].trim(),
        None => prompt,
    }
}

fn is_app_generated(prompt: &str) -> bool {
    person_words(prompt).trim_start().starts_with(APP_GENERATED)
}

// The app's own turns (their steps and ending change nothing either), the
// sessions it began for them alone (nothing of theirs counts until the
// person speaks in one), and the sessions the person spoke in: the latest
// KEPT of each, for as long as the app reading them runs.
struct Seen {
    app_turns: VecDeque<String>,
    app_sessions: VecDeque<String>,
    persons: VecDeque<String>,
}

const KEPT: usize = 64;
static SEEN: Mutex<Seen> = Mutex::new(Seen { app_turns: VecDeque::new(), app_sessions: VecDeque::new(), persons: VecDeque::new() });

fn keep(list: &mut VecDeque<String>, id: &str) {
    if id.is_empty() || list.iter().any(|x| x == id) {
        return;
    }
    if list.len() >= KEPT {
        list.pop_front();
    }
    list.push_back(id.to_string());
}

// An event as the app's own requests go: None for one read as it is;
// Some(None) for one that changes nothing (a step or the end of the app's
// turn, its turn in a session the person spoke in); Some(Some(kind)) for one
// to be that instead (its turn in a session begun for it: that session's
// end, so that it does not stay in the list with nothing in it).
fn as_app_asks(e: &Value, name: &str) -> Option<Option<Kind>> {
    let (session, turn) = (s(e, "session_id"), s(e, "turn_id"));
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if name == "UserPromptSubmit" {
        if !is_app_generated(s(e, "prompt")) {
            seen.app_sessions.retain(|x| x != session);
            keep(&mut seen.persons, session);
            return None;
        }
        keep(&mut seen.app_turns, turn);
        // A session not heard from since the app reading began is taken
        // for one begun for it.
        if seen.persons.iter().any(|x| x == session) {
            return Some(None);
        }
        keep(&mut seen.app_sessions, session);
        return Some(Some(Kind::SessionEnd));
    }
    let is_apps = (!turn.is_empty() && seen.app_turns.iter().any(|x| x == turn)) || seen.app_sessions.iter().any(|x| x == session);
    is_apps.then_some(None)
}

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
    // The desktop app's own requests: no doing of the person's.
    if let Some(instead) = as_app_asks(e, name) {
        return instead.map(|kind| event_of(e, kind));
    }
    let kind = match name {
        "SessionStart" if s(e, "source") == "compact" => return None,
        "SessionStart" => Kind::SessionStart,
        "SessionEnd" => Kind::SessionEnd,
        "UserPromptSubmit" => Kind::TurnStart { prompt: given(person_words(s(e, "prompt"))) },
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
    Some(event_of(e, kind))
}

fn event_of(e: &Value, kind: Kind) -> AgentEvent {
    AgentEvent {
        agent: Agent::Codex,
        session: s(e, "session_id").to_string(),
        within: false,
        cwd: given(s(e, "cwd")),
        turn: given(s(e, "turn_id")),
        title: None,
        reply: given(s(e, "last_assistant_message")),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{SessionRegistry, Status};
    use serde_json::json;

    #[test]
    fn the_files_a_patch_touches() {
        let patch = "*** Begin Patch\n*** Add File: hello.txt\n+hi\n*** Update File: D:\\w\\b.txt\n@@\n-a\n+b\n*** Delete File: old.txt\n*** End Patch";
        assert_eq!(patched(patch), ["hello.txt", "D:\\w\\b.txt", "old.txt"]);
        assert!(patched("not a patch").is_empty());
        // A patch done: every file it changed, whole (zhoupengjie/agent-hub#2).
        let done = json!({ "hook_event_name": "PostToolUse", "session_id": "s", "cwd": "D:\\w", "tool_name": "apply_patch", "tool_input": { "command": patch } });
        let Some(AgentEvent { kind: Kind::ToolDone { call }, .. }) = to_event(&done) else { panic!("no tool done") };
        assert_eq!((call.file.as_deref(), call.files.as_slice()), (Some("D:\\w\\hello.txt"), &["D:\\w\\hello.txt".to_string(), "D:\\w\\b.txt".into(), "D:\\w\\old.txt".into()][..]));
    }

    // The person's words, as the desktop app hands them on with what they
    // attached (Codex Desktop 26.1013; zhoupengjie/Wakuwaku#74).
    #[test]
    fn the_person_words_in_a_prompt() {
        assert_eq!(person_words("fix the login page"), "fix the login page");
        let image = "# Files mentioned by the user:\n\n## shot.png: C:\\w\\shot.png\nImage attachment: true\n\n## My request for Codex:\nwhy is this button grey?\n";
        assert_eq!(person_words(image), "why is this button grey?");
        let comment = "<page-comment-context>\nthe header\n</page-comment-context>\n## My request:\nmake it blue";
        assert_eq!(person_words(comment), "make it blue");
        // Only an image, no words: no prompt, the name it had stays.
        let only = "# Files mentioned by the user:\n\n## a.png: C:\\w\\a.png\nImage attachment: true\n\n## My request for Codex:\n";
        assert_eq!(to_event(&hook("UserPromptSubmit", "words-74", "w1", json!({ "prompt": only }))).unwrap().kind, Kind::TurnStart { prompt: None });
        assert_eq!(to_event(&hook("UserPromptSubmit", "words-74", "w2", json!({ "prompt": image }))).unwrap().kind, Kind::TurnStart { prompt: Some("why is this button grey?".into()) });
    }

    fn hook(name: &str, session: &str, turn: &str, more: Value) -> Value {
        let mut e = json!({ "hook_event_name": name, "session_id": session, "turn_id": turn, "cwd": "D:\\w\\pet" });
        for (k, v) in more.as_object().unwrap() {
            e[k] = v.clone();
        }
        e
    }

    const SUGGEST: &str = "This is an app-generated request for a suggested next user message. Reply with one short message.";

    // An event read and kept in the registry: what it was read as.
    fn feed(r: &mut SessionRegistry, e: Value) -> Option<Kind> {
        let ev = to_event(&e)?;
        r.apply(&ev, 1);
        Some(ev.kind)
    }

    // The desktop app's own request for a suggested next message, in a
    // session begun for it: the session gone, its steps and ending nothing;
    // it counts once the person speaks in it (zhoupengjie/Wakuwaku#74).
    #[test]
    fn the_app_asking_in_a_session_of_its_own() {
        let r = &mut SessionRegistry::default();
        assert_eq!(feed(r, hook("SessionStart", "app-74", "", json!({ "source": "startup" }))), Some(Kind::SessionStart));
        assert_eq!(feed(r, hook("UserPromptSubmit", "app-74", "a1", json!({ "prompt": SUGGEST }))), Some(Kind::SessionEnd));
        assert_eq!(feed(r, hook("PreToolUse", "app-74", "a1", json!({ "tool_name": "Bash" }))), None);
        assert_eq!(feed(r, hook("Stop", "app-74", "a1", json!({ "last_assistant_message": "Looks good, thanks!" }))), None);
        assert_eq!(feed(r, hook("SessionEnd", "app-74", "", json!({}))), None);
        assert_eq!(r.sessions().count(), 0);
        feed(r, hook("UserPromptSubmit", "app-74", "p1", json!({ "prompt": "now add a test" }))).unwrap();
        feed(r, hook("Stop", "app-74", "p1", json!({ "last_assistant_message": "Added." }))).unwrap();
        let s = r.get("app-74").unwrap();
        assert_eq!((s.prompt.as_deref(), s.reply.as_deref()), (Some("now add a test"), Some("Added.")));
    }

    // In a session the person spoke in: that turn alone left out, the
    // session as it was.
    #[test]
    fn the_app_asking_in_the_person_session() {
        let r = &mut SessionRegistry::default();
        feed(r, hook("UserPromptSubmit", "mine-74", "m1", json!({ "prompt": "fix the login page" }))).unwrap();
        feed(r, hook("Stop", "mine-74", "m1", json!({ "last_assistant_message": "Fixed." }))).unwrap();
        assert_eq!(feed(r, hook("UserPromptSubmit", "mine-74", "m2", json!({ "prompt": SUGGEST }))), None);
        assert_eq!(feed(r, hook("Stop", "mine-74", "m2", json!({ "last_assistant_message": "Thanks!" }))), None);
        let s = r.get("mine-74").unwrap();
        assert_eq!((&s.status, s.prompt.as_deref(), s.reply.as_deref()), (&Status::Done { edited: false }, Some("fix the login page"), Some("Fixed.")));
        // The person's next words, whatever they are, its name now.
        feed(r, hook("UserPromptSubmit", "mine-74", "m3", json!({ "prompt": "thanks, all good" }))).unwrap();
        assert_eq!(r.get("mine-74").unwrap().prompt.as_deref(), Some("thanks, all good"));
    }
}
