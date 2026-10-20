// An agent's event as what a session is doing, in no agent's words: what each
// agent's adapter (claude.rs, codex.rs, ...: to_event) makes of what the
// agent sends, for an app to show its own way (Wakuwaku's moods and words)
// and to keep (the sessions it knows). Serialized as an app would be sent
// it: camelCase, each kind by its "type".
use serde::Serialize;
use serde_json::Value;

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
    // The file a finished edit changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    // A change it makes to the session's to-do list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo: Option<Todo>,
}

impl Call {
    pub fn new(id: &str, tool: &str, input: Value) -> Call {
        Call { id: (!id.is_empty()).then(|| id.to_string()), tool: tool.to_string(), own: None, input, raw: None, intent: None, file: None, todo: None }
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
    Question,
    // A plan to confirm (ExitPlanMode).
    Plan,
    // An MCP server asks for input (Elicitation).
    Input { server: String },
    // Its attention, in its words or none.
    Notice { said: Option<String> },
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
