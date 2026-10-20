// An agent's event as what a session is doing, in no agent's words: what each
// agent's adapter (claude.rs, codex.rs, ...: to_event) makes of what the
// agent sends, for an app to show its own way (Wakuwaku's moods and words)
// and to keep (the sessions it knows). Serialized as an app would be sent
// it: camelCase, each kind by its "type"; read back so (from_json: an app
// may be given one, Wakuwaku's POST /state).
use serde::Serialize;
use serde_json::{Map, Value};

impl AgentEvent {
    // Its call, where it has one (a tool's, or one waiting for approval).
    pub fn call_mut(&mut self) -> Option<&mut Call> {
        match &mut self.kind {
            Kind::ToolStart { call } | Kind::ToolDone { call } | Kind::ToolFailed { call, .. } => Some(call),
            Kind::Waiting { wait: Wait::Approval { what: What::Call { call }, .. } } => Some(call),
            _ => None,
        }
    }
}

// Whose event: the agents she follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
    Agy,
    Opencode,
    Omp,
    Zcode,
    Reasonix,
    Dsh,
}

impl Agent {
    // By the name its events come under (/hook?agent=...; Claude Code's
    // under none).
    pub fn from_tag(tag: Option<&str>) -> Agent {
        match tag {
            Some("codex") => Agent::Codex,
            Some("agy") => Agent::Agy,
            Some("opencode") => Agent::Opencode,
            Some("omp") => Agent::Omp,
            Some("zcode") => Agent::Zcode,
            Some("reasonix") => Agent::Reasonix,
            Some("dsh") => Agent::Dsh,
            _ => Agent::Claude,
        }
    }

    // That name (Claude Code's: none).
    pub fn tag(self) -> Option<&'static str> {
        match self {
            Agent::Claude => None,
            Agent::Codex => Some("codex"),
            Agent::Agy => Some("agy"),
            Agent::Opencode => Some("opencode"),
            Agent::Omp => Some("omp"),
            Agent::Zcode => Some("zcode"),
            Agent::Reasonix => Some("reasonix"),
            Agent::Dsh => Some("dsh"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub agent: Agent,
    // The session ("" when the agent named none).
    pub session: String,
    // A subagent's (its planner's, its guardian's) event, told as the
    // session's it works for: only into a turn of that one's under way; it
    // never starts a session, nor wakes one at rest or ended.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub within: bool,
    // The folder it works in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    // The turn it belongs to, where the agent says (Codex's turn_id,
    // Antigravity's step of the person's words a turn answers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    // The session's name, when this event tells it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    // The agent's last words, whole, when the event carries them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Kind {
    SessionStart,
    SessionEnd,
    // A turn begins; the person's words when they are the person's (not a
    // turn the system began).
    TurnStart { prompt: Option<String> },
    ToolStart { call: Call },
    ToolDone { call: Call },
    // `quiet`: said of no reaction of its own (a subagent's).
    ToolFailed { call: Call, quiet: bool },
    Waiting { wait: Wait },
    // At work again (a prompt answered, the model called again): no turn,
    // no tool of its own.
    Working,
    // At rest, no turn ended (stopped before it began, a plan put away).
    Rest,
    // The person stopped the turn.
    Stopped,
    Done,
    Failed { error: Failure },
    // A task of its to-do list done (Claude Code's TaskCompleted).
    TaskDone { subject: Option<String> },
}

// A tool call.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Call {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    // By Claude Code's name for it where there is one (Codex's apply_patch,
    // Antigravity's own names, an MCP tool's mcp__<server>__<tool> as they are).
    pub tool: String,
    // The agent's own name for it, where the tool goes by Claude Code's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own: Option<String>,
    // Its input as Claude Code's tool takes it (file_path, command, ...).
    pub input: Value,
    // Its input as the agent gave it, where that differs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
    // The model's words for what it does (omp's intent, Antigravity's toolAction).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    // The file a finished edit changed (a patch's first), and every one a
    // patch changed (Codex's apply_patch, OpenCode's patch); by their whole
    // paths.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    // A change it makes to the session's to-do list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo: Option<Todo>,
    // Its input was cut short before it came (the app's plugin in the agent
    // keeps an event's size down): not all of it is here.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl Call {
    pub fn new(id: &str, tool: &str, input: Value) -> Call {
        Call { id: (!id.is_empty()).then(|| id.to_string()), tool: tool.to_string(), own: None, input, raw: None, intent: None, file: None, files: Vec::new(), todo: None, truncated: false }
    }
}

// A change to a session's to-do list: the whole of it (TodoWrite), one more
// (TaskCreate, numbered from 1 as Claude Code numbers them), or one changed.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "camelCase")]
pub enum Todo {
    Set { items: Vec<TodoItem> },
    Add { item: TodoItem },
    Update { id: String, status: Option<String>, text: Option<String>, active: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub text: String,
    // Its words while under way ("Running tests").
    pub active: String,
    pub status: String,
}

// What a session waits on the person for.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "for", rename_all = "camelCase")]
pub enum Wait {
    Approval {
        what: What,
        #[serde(rename = "answerIn")]
        answer_in: AnswerIn,
    },
    // A question; where it may be answered and its words, where they are
    // told (one an app handed back to the agent's own window).
    Question {
        #[serde(skip_serializing_if = "Option::is_none")]
        question: Option<String>,
        #[serde(rename = "answerIn", skip_serializing_if = "Option::is_none")]
        answer_in: Option<AnswerIn>,
    },
    // A plan to confirm (ExitPlanMode).
    Plan,
    // An MCP server asks for input (Elicitation).
    Input { server: String },
    // Its attention, in its words or none.
    Notice { said: Option<String> },
}

impl Wait {
    // A question, as an agent's hook tells it (no words, answered wherever).
    pub fn question() -> Wait {
        Wait::Question { question: None, answer_in: None }
    }
}

// What an approval is for: a call, an action of the agent's own with no
// call (OpenCode's external_directory), or only the agent's words for it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "as", rename_all = "camelCase")]
pub enum What {
    Call { call: Call },
    Action { name: String },
    Said { said: String },
}

// Where it may be answered: by whoever holds the hook (an app), only in an
// app of the agent's own, or only in the terminal it runs in.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "in", rename_all = "camelCase")]
pub enum AnswerIn {
    Here,
    App { name: String },
    Terminal,
}

// Why a turn failed: one of Claude Code's kinds of error (rate_limit,
// authentication_failed, ...), or the agent's own words.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "as", rename_all = "camelCase")]
pub enum Failure {
    Kind { kind: String },
    Said { said: String },
}

// Text given, or None for none.
pub(crate) fn given(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

// An event read back from its JSON as serialized above: what it leaves out
// (or gives as null) taken as none, what it adds passed over (/v1/events'
// seq and at). By hand, not by serde's Deserialize: that, for these tagged
// kinds, came to some 110 KB in an exe that only reads them here.
impl AgentEvent {
    pub fn from_json(v: &Value) -> Result<AgentEvent, String> {
        let o = fields(v, "an event")?;
        let agent = text(o, "agent")?;
        let agent = Agent::named(&agent).ok_or(format!("no agent {agent:?}"))?;
        Ok(AgentEvent {
            agent,
            session: text(o, "session")?,
            within: match o.get("within") {
                None | Some(Value::Null) => false,
                Some(within) => within.as_bool().ok_or("within: not true or false")?,
            },
            cwd: maybe(o, "cwd")?,
            turn: maybe(o, "turn")?,
            title: maybe(o, "title")?,
            reply: maybe(o, "reply")?,
            kind: Kind::from_json(o)?,
        })
    }
}

impl Agent {
    // By its name as serialized ("claude" too, as from_tag has it not).
    pub fn named(name: &str) -> Option<Agent> {
        [Agent::Claude, Agent::Codex, Agent::Agy, Agent::Opencode, Agent::Omp, Agent::Zcode, Agent::Reasonix, Agent::Dsh].into_iter().find(|a| a.tag().unwrap_or("claude") == name)
    }
}

impl Kind {
    fn from_json(o: &Map<String, Value>) -> Result<Kind, String> {
        Ok(match text(o, "type")?.as_str() {
            "sessionStart" => Kind::SessionStart,
            "sessionEnd" => Kind::SessionEnd,
            "turnStart" => Kind::TurnStart { prompt: maybe(o, "prompt")? },
            "toolStart" => Kind::ToolStart { call: Call::from_json(within(o, "call")?)? },
            "toolDone" => Kind::ToolDone { call: Call::from_json(within(o, "call")?)? },
            "toolFailed" => Kind::ToolFailed { call: Call::from_json(within(o, "call")?)?, quiet: o.get("quiet").and_then(Value::as_bool).unwrap_or(false) },
            "waiting" => Kind::Waiting { wait: Wait::from_json(within(o, "wait")?)? },
            "working" => Kind::Working,
            "rest" => Kind::Rest,
            "stopped" => Kind::Stopped,
            "done" => Kind::Done,
            "failed" => Kind::Failed { error: Failure::from_json(within(o, "error")?)? },
            "taskDone" => Kind::TaskDone { subject: maybe(o, "subject")? },
            other => return Err(format!("no type {other:?}")),
        })
    }
}

impl Call {
    fn from_json(o: &Map<String, Value>) -> Result<Call, String> {
        Ok(Call {
            id: maybe(o, "id")?,
            tool: text(o, "tool")?,
            own: maybe(o, "own")?,
            input: o.get("input").cloned().unwrap_or(Value::Null),
            raw: o.get("raw").filter(|r| !r.is_null()).cloned(),
            intent: maybe(o, "intent")?,
            file: maybe(o, "file")?,
            files: match o.get("files") {
                None | Some(Value::Null) => Vec::new(),
                Some(files) => files.as_array().ok_or("files: not a list")?.iter().map(|f| f.as_str().map(str::to_string).ok_or("files: not text")).collect::<Result<_, _>>()?,
            },
            todo: match o.get("todo") {
                None | Some(Value::Null) => None,
                Some(todo) => Some(Todo::from_json(fields(todo, "todo")?)?),
            },
            truncated: o.get("truncated").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

impl Todo {
    fn from_json(o: &Map<String, Value>) -> Result<Todo, String> {
        let item = |v: &Value| -> Result<TodoItem, String> {
            let o = fields(v, "an item")?;
            let part = |key| Ok::<_, String>(maybe(o, key)?.unwrap_or_default());
            Ok(TodoItem { text: part("text")?, active: part("active")?, status: part("status")? })
        };
        Ok(match text(o, "change")?.as_str() {
            "set" => Todo::Set { items: o.get("items").and_then(Value::as_array).ok_or("items: not a list")?.iter().map(item).collect::<Result<_, _>>()? },
            "add" => Todo::Add { item: item(o.get("item").unwrap_or(&Value::Null))? },
            "update" => Todo::Update { id: text(o, "id")?, status: maybe(o, "status")?, text: maybe(o, "text")?, active: maybe(o, "active")? },
            other => return Err(format!("no change {other:?}")),
        })
    }
}

impl Wait {
    fn from_json(o: &Map<String, Value>) -> Result<Wait, String> {
        Ok(match text(o, "for")?.as_str() {
            "approval" => Wait::Approval { what: What::from_json(within(o, "what")?)?, answer_in: AnswerIn::from_json(within(o, "answerIn")?)? },
            "question" => Wait::Question {
                question: maybe(o, "question")?,
                answer_in: match o.get("answerIn") {
                    None | Some(Value::Null) => None,
                    Some(answer_in) => Some(AnswerIn::from_json(fields(answer_in, "answerIn")?)?),
                },
            },
            "plan" => Wait::Plan,
            "input" => Wait::Input { server: text(o, "server")? },
            "notice" => Wait::Notice { said: maybe(o, "said")? },
            other => return Err(format!("no wait for {other:?}")),
        })
    }
}

impl What {
    fn from_json(o: &Map<String, Value>) -> Result<What, String> {
        Ok(match text(o, "as")?.as_str() {
            "call" => What::Call { call: Call::from_json(within(o, "call")?)? },
            "action" => What::Action { name: text(o, "name")? },
            "said" => What::Said { said: text(o, "said")? },
            other => return Err(format!("no what as {other:?}")),
        })
    }
}

impl AnswerIn {
    fn from_json(o: &Map<String, Value>) -> Result<AnswerIn, String> {
        Ok(match text(o, "in")?.as_str() {
            "here" => AnswerIn::Here,
            "app" => AnswerIn::App { name: text(o, "name")? },
            "terminal" => AnswerIn::Terminal,
            other => return Err(format!("no answer in {other:?}")),
        })
    }
}

impl Failure {
    fn from_json(o: &Map<String, Value>) -> Result<Failure, String> {
        Ok(match text(o, "as")?.as_str() {
            "kind" => Failure::Kind { kind: text(o, "kind")? },
            "said" => Failure::Said { said: text(o, "said")? },
            other => return Err(format!("no failure as {other:?}")),
        })
    }
}

// An object's fields.
fn fields<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    v.as_object().ok_or(format!("{what}: not an object"))
}

// The object a field holds.
fn within<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a Map<String, Value>, String> {
    fields(o.get(key).ok_or(format!("no {key}"))?, key)
}

// A field's text, which must be there.
fn text(o: &Map<String, Value>, key: &str) -> Result<String, String> {
    maybe(o, key)?.ok_or(format!("no {key}"))
}

// A field's text, or none (left out, or null).
fn maybe(o: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{key}: not text")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // What is sent is read back the same, and what an event leaves out is none.
    #[test]
    fn an_event_is_read_back_as_it_was_sent() {
        let call = Call {
            raw: Some(json!({ "path": "a.md" })),
            intent: Some("note it".into()),
            file: Some("Z:/p/a.md".into()),
            files: vec!["Z:/p/a.md".into(), "Z:/p/b.md".into()],
            truncated: true,
            todo: Some(Todo::Update { id: "2".into(), status: Some("completed".into()), text: None, active: None }),
            ..Call::new("t1", "Write", json!({ "file_path": "Z:/p/a.md", "content": "hi" }))
        };
        let base = AgentEvent { agent: Agent::Codex, session: "s".into(), within: true, cwd: Some("Z:/p".into()), turn: Some("u1".into()), title: Some("notes".into()), reply: Some("Done.".into()), kind: Kind::Working };
        let kinds = [
            Kind::SessionStart,
            Kind::SessionEnd,
            Kind::TurnStart { prompt: Some("write it".into()) },
            Kind::TurnStart { prompt: None },
            Kind::ToolStart { call: call.clone() },
            Kind::ToolDone { call: call.clone() },
            Kind::ToolFailed { call: Call::new("", "Bash", json!({})), quiet: true },
            Kind::Waiting { wait: Wait::Approval { what: What::Call { call: call.clone() }, answer_in: AnswerIn::App { name: "DeepSeek Harness".into() } } },
            Kind::Waiting { wait: Wait::Approval { what: What::Said { said: "rm".into() }, answer_in: AnswerIn::Terminal } },
            Kind::Waiting { wait: Wait::question() },
            Kind::Waiting { wait: Wait::Question { question: Some("Which?".into()), answer_in: Some(AnswerIn::Here) } },
            Kind::Waiting { wait: Wait::Input { server: "github".into() } },
            Kind::Waiting { wait: Wait::Notice { said: None } },
            Kind::Rest,
            Kind::Stopped,
            Kind::Done,
            Kind::Failed { error: Failure::Kind { kind: "rate_limit".into() } },
            Kind::TaskDone { subject: Some("tests".into()) },
        ];
        for kind in kinds {
            let ev = AgentEvent { kind, ..base.clone() };
            let sent = serde_json::to_value(&ev).unwrap();
            assert_eq!(AgentEvent::from_json(&sent), Ok(ev), "{sent}");
        }
        // A question as before: no words, nowhere said.
        assert_eq!(serde_json::to_value(Wait::question()).unwrap(), json!({ "for": "question" }));
        // The least there is; what /v1/events adds is passed over.
        let ev = AgentEvent::from_json(&json!({ "agent": "claude", "session": "a", "type": "toolFailed", "call": { "tool": "Bash" }, "cwd": null, "seq": 3, "at": 9 })).unwrap();
        assert_eq!(ev, AgentEvent { agent: Agent::Claude, session: "a".into(), within: false, cwd: None, turn: None, title: None, reply: None, kind: Kind::ToolFailed { call: Call { input: Value::Null, ..Call::new("", "Bash", Value::Null) }, quiet: false } });
        for wrong in [json!({ "agent": "claude", "session": "a" }), json!({ "agent": "nobody", "session": "a", "type": "done" }), json!({ "session": "a", "type": "done" }), json!({ "agent": "claude", "session": "a", "type": "sleep" }), json!({ "agent": "claude", "session": 7, "type": "done" }), json!({ "agent": "claude", "session": "a", "type": "waiting", "wait": { "for": "approval", "what": { "as": "said", "said": "x" } } }), json!("done")] {
            assert!(AgentEvent::from_json(&wrong).is_err(), "{wrong}");
        }
    }
}
