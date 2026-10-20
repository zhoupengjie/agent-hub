// What an app that follows the agents' sessions tells other local apps of
// them (an editor, a script): which sessions there are, and each event as it
// comes, through its own HTTP port (the app serves the requests; this crate
// knows no HTTP server):
//
//   GET /v1/info       the hub: its API version, what it can tell, the app
//                      it runs in, its start (boot), its latest event's
//                      number, where its token is (no token needed)
//   GET /v1/sessions   the sessions known (agent-core's registry): ?cwd=<a
//                      folder> (those working in it or under it), ?agent=
//                      codex,dsh (an agent it does not know passed over;
//                      none known, an error); with the latest event's
//                      number, so that /v1/events?since= takes on from them
//                      with none lost
//   GET /v1/events     Server-Sent Events: each AgentEvent with its number
//                      ("seq"; its id: is boot:seq) and time ("at"), a
//                      turn's end (done, stopped, failed) with the files the
//                      turn changed ("files"); ?since=<n> or <boot:n> (or a
//                      Last-Event-ID) sends those after n first, from the
//                      last KEPT (KEPT_BYTES at most); when some were lost
//                      (too old, or the hub started again since: its
//                      numbers began anew, its boot another), an event
//                      "reset" first, to read /v1/sessions again;
//                      an event "sessions" when work that never said it
//                      ended was given up, or quiet sessions forgotten
//                      (tick), to read it again too; the same ?cwd= and
//                      ?agent=; ": ping" every 15 s
//
// /v1/sessions and /v1/events take the hub's token (Authorization: Bearer
// <token>), kept in a file of the app's own folder (token_file): the events
// carry what the person asked and the files' text an edit changed, and a web
// page in a browser on this machine could otherwise ask for them.
//
// One app holds the hub at a time: the one on PORT (another that finds it
// taken is a client of it). A client finds it there (or at AGENT_HUB_URL:
// url()), asks /v1/info for the API and the token's file, reads the token,
// then asks the rest; nothing is written anywhere else to find it by. A
// subscriber that does not read keeps ROOM events waiting at most, then is
// let go: a slow reader never holds up the agents' hooks.
use std::collections::hash_map::RandomState;
use std::collections::VecDeque;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::event::{Agent, AgentEvent, Kind};
use agent_core::registry::SessionRegistry;
use serde_json::{json, Value};

// Where the hub is: this port of this machine's, unless a client is told
// otherwise (URL_VAR, an environment variable: a test's, another port).
pub const PORT: u16 = 47213;
pub const URL_VAR: &str = "AGENT_HUB_URL";

// The address a client asks the hub at.
pub fn url() -> String {
    std::env::var(URL_VAR).ok().map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty()).unwrap_or_else(|| format!("http://127.0.0.1:{PORT}"))
}

// The API's version: /v2 for a change that is not only something added.
pub const API: u32 = 1;
// What /v1 tells (a client looks here before asking).
pub const CAPABILITIES: [&str; 2] = ["sessions", "events"];
// The events kept for a client that comes back (?since=), and their bytes at
// most: an event may carry a whole file (a Write's content), and they are
// kept whether anybody listens or not.
const KEPT: usize = 512;
const KEPT_BYTES: usize = 8 * 1024 * 1024;
// The events a subscriber may leave unread before it is let go.
const ROOM: usize = 256;
// How often an idle stream says it is there (and finds a client gone).
const PING: Duration = Duration::from_secs(15);

// Which sessions a client wants: those in a folder (or under it), of these agents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    cwd: Option<String>,
    agents: Vec<Agent>,
}

impl Filter {
    // From a query string (cwd=…&agent=codex,dsh).
    // An agent it does not know is passed over (one a newer client knows,
    // asked of an older hub); asked for none it knows, it is an error, not
    // every agent's sessions.
    pub fn from_query(query: &str) -> Result<Filter, String> {
        let mut filter = Filter::default();
        let mut unknown = Vec::new();
        for (key, value) in pairs(query) {
            match key.as_str() {
                "cwd" if !value.is_empty() => filter.cwd = Some(same_path(&value)),
                "agent" => {
                    for name in value.split(',').filter(|a| !a.is_empty()) {
                        match Agent::named(name) {
                            Some(agent) => filter.agents.push(agent),
                            None => unknown.push(name.to_string()),
                        }
                    }
                }
                _ => {}
            }
        }
        if filter.agents.is_empty() && !unknown.is_empty() {
            return Err(format!("no agent {} (claude, codex, agy, opencode, omp, zcode, reasonix, dsh)", unknown.join(", ")));
        }
        Ok(filter)
    }

    fn admits(&self, agent: Agent, cwd: Option<&str>) -> bool {
        let in_folder = match (&self.cwd, cwd) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(want), Some(cwd)) => {
                let cwd = same_path(cwd);
                cwd == *want || cwd.starts_with(&format!("{want}/"))
            }
        };
        in_folder && (self.agents.is_empty() || self.agents.contains(&agent))
    }
}

// A path as paths are compared: one kind of slash, none at the end, and on
// Windows any case.
fn same_path(path: &str) -> String {
    let path = path.replace('\\', "/").trim_end_matches('/').to_string();
    if cfg!(windows) { path.to_lowercase() } else { path }
}

// Where a client takes on from: `seq`, or `boot:seq` (an event's id).
fn position(text: &str) -> Option<(Option<u64>, u64)> {
    let text = text.trim();
    match text.split_once(':') {
        Some((boot, seq)) => Some((Some(boot.parse().ok()?), seq.parse().ok()?)),
        None => Some((None, text.parse().ok()?)),
    }
}

// A query string's pairs, their values %-decoded.
fn pairs(query: &str) -> Vec<(String, String)> {
    query.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').unwrap_or((p, ""))).map(|(k, v)| (decode(k), decode(v))).collect()
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push((high * 16 + low) as u8);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// An event as sent: its number, its folder (the session's, where the event
// gave none) for the filters, and its lines on the stream (one copy, shared
// by the kept and every subscriber's queue).
struct Record {
    seq: u64,
    agent: Agent,
    cwd: Option<String>,
    chunk: Arc<str>,
}

struct Subscriber {
    filter: Filter,
    tx: SyncSender<Arc<str>>,
}

struct State {
    registry: SessionRegistry,
    seq: u64,
    kept: VecDeque<Record>,
    kept_bytes: usize,
    subscribers: Vec<Subscriber>,
}

pub struct Hub {
    token: String,
    token_file: PathBuf,
    // When it started (ms): a client that saw another start knows its numbers began anew.
    boot: u64,
    state: Mutex<State>,
}

// A new token: 128 bits from the system's randomness (std's RandomState
// keys come from it, each instance's its own).
pub fn new_token() -> String {
    (0..2).map(|_| format!("{:016x}", RandomState::new().build_hasher().finish())).collect()
}

impl Hub {
    // The hub, its token in `folder` (made, with the folder, when there is none).
    pub fn open(folder: &Path, now: u64) -> io::Result<Hub> {
        std::fs::create_dir_all(folder)?;
        let token_file = folder.join("token");
        let token = match std::fs::read_to_string(&token_file) {
            Ok(text) if text.trim().len() >= 32 => text.trim().to_string(),
            _ => {
                let token = new_token();
                std::fs::write(&token_file, &token)?;
                token
            }
        };
        Ok(Hub::with_token(token, token_file, now))
    }

    pub fn with_token(token: String, token_file: PathBuf, now: u64) -> Hub {
        Hub { token, token_file, boot: now, state: Mutex::new(State { registry: SessionRegistry::default(), seq: 0, kept: VecDeque::new(), kept_bytes: 0, subscribers: Vec::new() }) }
    }

    pub fn token_file(&self) -> &Path {
        &self.token_file
    }

    // Whether a request's Authorization header carries the token.
    pub fn allows(&self, authorization: Option<&str>) -> bool {
        let Some(given) = authorization.and_then(|a| a.trim().strip_prefix("Bearer ")) else { return false };
        let given = given.trim().as_bytes();
        // Every byte compared, whatever differs first.
        given.len() == self.token.len() && given.iter().zip(self.token.as_bytes()).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
    }

    // An event heard at `now`: into the sessions, kept, and sent to each
    // subscriber it is for. Its number.
    pub fn publish(&self, event: &AgentEvent, now: u64) -> u64 {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.registry.apply(event, now);
        let cwd = event.cwd.clone().or_else(|| st.registry.get(&event.session).and_then(|s| s.cwd.clone()));
        st.seq += 1;
        let seq = st.seq;
        let mut sent = serde_json::to_value(event).unwrap_or(Value::Null);
        sent["seq"] = json!(seq);
        sent["at"] = json!(now);
        if let Some(cwd) = &cwd {
            sent["cwd"] = json!(cwd);
        }
        // A turn's end with the files it changed (the session's).
        if matches!(event.kind, Kind::Done | Kind::Stopped | Kind::Failed { .. }) {
            if let Some(s) = st.registry.get(&event.session) {
                sent["files"] = json!(s.files);
            }
        }
        // Its id says this start of the hub's too (`boot:seq`): a client that
        // comes back with it from another start is told so (events).
        let chunk: Arc<str> = format!("id: {}:{seq}\nevent: agent\ndata: {sent}\n\n", self.boot).into();
        // A subscriber gone, or one that let ROOM go unread, is let go.
        st.subscribers.retain(|s| !s.filter.admits(event.agent, cwd.as_deref()) || s.tx.try_send(chunk.clone()).is_ok());
        st.kept_bytes += chunk.len();
        st.kept.push_back(Record { seq, agent: event.agent, cwd, chunk });
        while st.kept.len() > 1 && (st.kept.len() > KEPT || st.kept_bytes > KEPT_BYTES) {
            if let Some(gone) = st.kept.pop_front() {
                st.kept_bytes -= gone.chunk.len();
            }
        }
        seq
    }

    // Work that never said it ended given up, quiet sessions forgotten; each
    // subscriber told so (an event "sessions"). True when any was.
    pub fn tick(&self, now: u64) -> bool {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.registry.check_stale(now).is_empty() {
            return false;
        }
        let chunk: Arc<str> = format!("event: sessions\ndata: {}\n\n", json!({ "seq": st.seq })).into();
        st.subscribers.retain(|s| s.tx.try_send(chunk.clone()).is_ok());
        true
    }

    // GET /v1/info, for the app `app` at `version`.
    pub fn info(&self, app: &str, version: &str) -> Value {
        let seq = self.state.lock().unwrap_or_else(|e| e.into_inner()).seq;
        json!({
            "api": API,
            "hub": env!("CARGO_PKG_VERSION"),
            "app": app,
            "appVersion": version,
            "boot": self.boot,
            "seq": seq,
            "capabilities": CAPABILITIES,
            "token": { "header": "Authorization: Bearer <token>", "file": self.token_file },
        })
    }

    // GET /v1/sessions?<query>.
    pub fn sessions(&self, query: &str) -> Result<Value, String> {
        let filter = Filter::from_query(query)?;
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let sessions: Vec<Value> = st.registry.list().into_iter().filter(|s| filter.admits(s.agent, s.cwd.as_deref())).map(|s| serde_json::to_value(s).unwrap_or(Value::Null)).collect();
        Ok(json!({ "boot": self.boot, "seq": st.seq, "sessions": sessions }))
    }

    // GET /v1/events?<query>: the stream to answer with (its body), from
    // ?since=, else a Last-Event-ID, else from now on.
    pub fn events(&self, query: &str, last_event_id: Option<&str>) -> Result<Events, String> {
        let filter = Filter::from_query(query)?;
        let since = pairs(query).into_iter().find(|(k, _)| k == "since").and_then(|(_, v)| position(&v)).or_else(|| last_event_id.and_then(position));
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut first: Vec<Arc<str>> = Vec::new();
        if let Some((boot, since)) = since {
            let oldest = st.kept.front().map_or(st.seq + 1, |r| r.seq);
            // Another start of the hub's (its numbers began anew), numbers it
            // never gave, or some no longer kept.
            let other_start = boot.is_some_and(|boot| boot != self.boot);
            if other_start || since > st.seq || since + 1 < oldest {
                first.push(format!("event: reset\ndata: {}\n\n", json!({ "boot": self.boot, "seq": st.seq })).into());
            }
            if !other_start {
                first.extend(st.kept.iter().filter(|r| r.seq > since && filter.admits(r.agent, r.cwd.as_deref())).map(|r| r.chunk.clone()));
            }
        }
        let (tx, rx) = sync_channel(first.len() + ROOM);
        for chunk in first {
            let _ = tx.try_send(chunk);
        }
        st.subscribers.push(Subscriber { filter, tx });
        Ok(Events { rx, buf: Arc::from(""), at: 0, ping: PING })
    }

    // How many are listening (for her log).
    pub fn listening(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).subscribers.len()
    }
}

// One client's stream: the events for it as they come, a ping while there
// are none; its end when the hub lets it go. Read by whatever answers the
// request (a chunked body, each read sent at once).
pub struct Events {
    rx: Receiver<Arc<str>>,
    buf: Arc<str>,
    at: usize,
    ping: Duration,
}

impl Read for Events {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.at >= self.buf.len() {
            self.buf = match self.rx.recv_timeout(self.ping) {
                Ok(chunk) => chunk,
                Err(RecvTimeoutError::Timeout) => Arc::from(": ping\n\n"),
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            };
            self.at = 0;
        }
        let bytes = self.buf.as_bytes();
        let n = out.len().min(bytes.len() - self.at);
        out[..n].copy_from_slice(&bytes[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests;
