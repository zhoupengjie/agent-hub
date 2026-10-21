// What an app that follows the agents' sessions tells other local apps of
// them (an editor, a script): which sessions there are, and each event as it
// comes, through its own HTTP port (the app serves the requests; this crate
// knows no HTTP server):
//
//   GET /v1/info       the hub: its API version, what it can tell, the app
//                      it runs in, its start (boot), its latest event's
//                      number, where its token is (no token needed);
//                      ?nonce=<n>: its proof that it knows the token
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
// url()), asks /v1/info?nonce=<a random one> for the API and the token's
// file, reads the token (read_token: only a token's file), checks the
// proof (proves), then asks the rest; nothing is written anywhere else to
// find it by. Whatever else may hold the port while no hub does gets no
// file of the person's and no token so. What the hub tells a client is
// data to show, never what to do. A
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
pub const CAPABILITIES: [&str; 3] = ["sessions", "events", "proof"];
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
// keys come from it, each instance's its own), as 32 lowercase hex digits.
pub fn new_token() -> String {
    (0..2).map(|_| format!("{:016x}", RandomState::new().build_hasher().finish())).collect()
}

// Whether a text is a token's form: 32 lowercase hex digits, new_token's.
fn is_token(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// A hub's token as a client reads it, from the file /v1/info names
// (token.file), or None: then it is no hub's, and the client sends nothing.
// Whatever holds the port may name any file (one of the person's keys, for
// the client to read and send it), so only a token's is read: a file named
// token on this machine's own disks, itself and not a link, 64 bytes at
// most, holding a token's form (zhoupengjie/agent-hub#4). Before anything
// is opened the path alone is looked at (local_path): opening a network
// path (\\host\share\…, WebDAV) would already send the person's sign-in to
// it, and a pipe would be its server's to answer. Then the folders above
// it, one by one, none a link (no_linked_folder): a link to a network path
// may be made by any account where Windows' developer mode is on.
//
// Left open: a folder looked at may be changed into a link before the file
// is opened (by whoever chose the path, writing there). Closing that would
// take opening each folder by its parent's handle, links not followed
// (NtCreateFile, OBJ_DONT_REPARSE); not done.
pub fn read_token(path: &Path) -> Option<String> {
    if !local_path(path) || path.file_name()? != "token" || !no_linked_folder(path) {
        return None;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // The file itself, not where a link points (a local one may point to a
    // network path); on Unix, a FIFO not waited on.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    // What was opened, by its handle (no other look by the path, which may
    // change in between): a plain file, small.
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > 64 {
        return None;
    }
    let mut text = String::new();
    file.take(65).read_to_string(&mut text).ok()?;
    let token = text.trim();
    (text.len() <= 64 && is_token(token)).then(|| token.to_string())
}

// Whether a path is a whole one on this machine's own disks, by its text
// alone (nothing opened, nothing asked of the file system: that would reach
// a network path already). On Windows a drive's (C:\…, \\?\C:\…) or a
// volume's by the name Windows gives it (\\?\Volume{…}\…: one with no
// drive letter has only that), not a network path (\\host\share\…,
// \\?\UNC\…), a device's (\\.\pipe\…) or another verbatim one
// (\\?\GLOBALROOT\…).
fn local_path(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(path.components().next(), Some(Component::Prefix(p)) if match p.kind() {
            Prefix::Disk(_) | Prefix::VerbatimDisk(_) => true,
            Prefix::Verbatim(name) => volume_name(name),
            _ => false,
        })
    }
    #[cfg(not(windows))]
    {
        true
    }
}

// Whether a name is a volume's as Windows names it, Volume{GUID} (the
// mount manager's, made for this machine's volumes alone; a share is never
// one).
#[cfg(windows)]
fn volume_name(name: &std::ffi::OsStr) -> bool {
    let Some(guid) = name.to_str().and_then(|n| n.strip_prefix("Volume{")).and_then(|n| n.strip_suffix('}')) else { return false };
    guid.len() == 36 && guid.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() })
}

// A file's path by its volume's name (\\?\Volume{…}\…), links taken out:
// how a hub names its token's file where the drive's path would not be
// read, one through a folder a volume is mounted on (Windows takes that
// for a junction, and so does no_linked_folder).
#[cfg(windows)]
fn volume_path(path: &Path) -> Option<PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFinalPathNameByHandleW(file: *mut std::ffi::c_void, path: *mut u16, len: u32, flags: u32) -> u32;
    }
    const VOLUME_NAME_GUID: u32 = 0x1;
    let file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u16; 300];
    loop {
        // SAFETY: the handle is open for the call; buf holds len u16s.
        let n = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_GUID) } as usize;
        if n == 0 || n > 32_768 {
            return None;
        }
        if n < buf.len() {
            buf.truncate(n);
            return Some(PathBuf::from(std::ffi::OsString::from_wide(&buf)));
        }
        buf.resize(n, 0);
    }
}

// Whether a client would read a token's file by this path (its checks
// before it opens it).
#[cfg(windows)]
fn read_by(path: &Path) -> bool {
    local_path(path) && no_linked_folder(path)
}

// Whether every folder above a file, from the root down, is a plain folder:
// none a link, nor a junction (Rust takes both for links). Each looked at
// without following it (symlink_metadata), so the first link stops it
// before anything is reached through it; a folder not there is a no
// (zhoupengjie/agent-hub#4). A hub names its token's file by the path with
// its links taken out (token_file_named), so that its own is never turned
// away.
fn no_linked_folder(path: &Path) -> bool {
    use std::path::Component;
    let Some(parent) = path.parent() else { return false };
    let mut seen = PathBuf::new();
    for part in parent.components() {
        seen.push(part);
        if matches!(part, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        match std::fs::symlink_metadata(&seen) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            _ => return false,
        }
    }
    true
}

// What a hub that knows `token` answers a client's nonce with (/v1/info?
// nonce=: its proof): HMAC-SHA256 of the nonce under the token, as hex. A
// client checks it (proves) before it sends the token: something else on
// the port, naming the real token's file, cannot give it.
pub fn proof(token: &str, nonce: &str) -> String {
    hmac_sha256(token.as_bytes(), nonce.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

// Whether a hub's proof for a nonce shows it knows the token.
pub fn proves(token: &str, nonce: &str, given: &str) -> bool {
    same(proof(token, nonce).as_bytes(), given.trim().as_bytes())
}

// HMAC (RFC 2104) with SHA-256.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.map(|k| k ^ byte);
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(message).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

// Two byte strings the same, every byte compared, whatever differs first.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

// The nonces /v1/info proves its token with: up to this long.
const NONCE_MAX: usize = 256;

impl Hub {
    // The hub, its token in `folder` (made, with the folder, when there is none).
    pub fn open(folder: &Path, now: u64) -> io::Result<Hub> {
        std::fs::create_dir_all(folder)?;
        let token_file = folder.join("token");
        // One of a token's form kept; anything else made anew, so that a
        // client's check (read_token) never turns this hub away.
        let token = match std::fs::read_to_string(&token_file) {
            Ok(text) if text.len() <= 64 && is_token(text.trim()) => text.trim().to_string(),
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

    // The token's file as a client is told it (/v1/info): by its path with
    // links and junctions taken out (\\?\C:\…), which a client's check
    // (read_token: no linked folder) takes; where that path would still not
    // be read (through a folder a volume is mounted on), by its volume's
    // name (\\?\Volume{…}\…); as it is where neither can be had.
    pub fn token_file_named(&self) -> PathBuf {
        let Ok(named) = std::fs::canonicalize(&self.token_file) else { return self.token_file.clone() };
        #[cfg(windows)]
        if !read_by(&named) {
            if let Some(by_volume) = volume_path(&self.token_file).filter(|p| read_by(p)) {
                return by_volume;
            }
        }
        named
    }

    // Whether a request's Authorization header carries the token.
    pub fn allows(&self, authorization: Option<&str>) -> bool {
        let Some(given) = authorization.and_then(|a| a.trim().strip_prefix("Bearer ")) else { return false };
        same(given.trim().as_bytes(), self.token.as_bytes())
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
        self.info_for(app, version, "")
    }

    // GET /v1/info?<query>: with ?nonce= (up to NONCE_MAX long), the proof
    // that this hub knows its token (proof).
    pub fn info_for(&self, app: &str, version: &str, query: &str) -> Value {
        let seq = self.state.lock().unwrap_or_else(|e| e.into_inner()).seq;
        let mut info = json!({
            "api": API,
            "hub": env!("CARGO_PKG_VERSION"),
            "app": app,
            "appVersion": version,
            "boot": self.boot,
            "seq": seq,
            "capabilities": CAPABILITIES,
            "token": { "header": "Authorization: Bearer <token>", "file": self.token_file_named() },
        });
        if let Some((_, nonce)) = pairs(query).into_iter().find(|(k, v)| k == "nonce" && !v.is_empty() && v.len() <= NONCE_MAX) {
            info["proof"] = json!(proof(&self.token, &nonce));
        }
        info
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
