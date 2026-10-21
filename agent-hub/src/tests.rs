use std::io::Read;

use agent_core::event::{Agent, AgentEvent, Call, Kind};
use serde_json::{json, Value};

use super::*;

fn ev(agent: Agent, session: &str, cwd: Option<&str>, kind: Kind) -> AgentEvent {
    AgentEvent { agent, session: session.into(), within: false, cwd: cwd.map(str::to_string), turn: None, title: None, reply: None, kind }
}

fn hub() -> Hub {
    Hub::with_token("t".repeat(32), PathBuf::from("Z:\\nowhere\\agent-hub\\token"), 1_000)
}

// What a stream has to read now (pings left out), as (event, data) pairs.
fn drain(events: &mut Events) -> Vec<(String, Value)> {
    blocks(events).into_iter().map(|(_, event, data)| (event, data)).collect()
}

// The same, as (id, event, data).
fn blocks(events: &mut Events) -> Vec<(String, String, Value)> {
    events.ping = Duration::from_millis(1);
    let mut text = String::new();
    let mut buf = [0u8; 64];
    loop {
        let n = events.read(&mut buf).unwrap();
        let got = String::from_utf8_lossy(&buf[..n]).into_owned();
        if n == 0 || (got == ": ping\n\n" && events.at >= events.buf.len()) {
            break;
        }
        text.push_str(&got);
    }
    text.split("\n\n")
        .filter(|block| !block.trim().is_empty() && !block.starts_with(':'))
        .map(|block| {
            let field = |name: &str| block.lines().find_map(|l| l.strip_prefix(&format!("{name}: "))).unwrap_or("").to_string();
            (field("id"), field("event"), serde_json::from_str(&field("data")).unwrap_or(Value::Null))
        })
        .collect()
}

#[test]
fn the_token_is_asked_for_and_kept() {
    let hub = hub();
    assert!(hub.allows(Some(&format!("Bearer {}", "t".repeat(32)))));
    assert!(!hub.allows(Some(&format!("Bearer {}", "u".repeat(32)))));
    assert!(!hub.allows(Some("Bearer t")));
    assert!(!hub.allows(None));
    assert!(!hub.allows(Some(&"t".repeat(32))));
    // Made once in its folder, then the same.
    let dir = std::env::temp_dir().join(format!("agent-hub-token-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let one = Hub::open(&dir, 0).unwrap();
    let token = std::fs::read_to_string(dir.join("token")).unwrap();
    assert_eq!(token.len(), 32);
    assert!(one.allows(Some(&format!("Bearer {token}"))));
    let two = Hub::open(&dir, 0).unwrap();
    assert!(two.allows(Some(&format!("Bearer {token}"))));
    assert_ne!(new_token(), new_token());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn events_come_numbered_to_those_who_want_them() {
    let hub = hub();
    let mut all = hub.events("", None).unwrap();
    let mut web = hub.events("cwd=Z%3A%5Creplay%5Cweb", None).unwrap();
    let mut codex = hub.events("agent=codex", None).unwrap();
    hub.publish(&ev(Agent::Claude, "a", Some("Z:\\replay\\web"), Kind::TurnStart { prompt: Some("fix it".into()) }), 2_000);
    hub.publish(&ev(Agent::Codex, "c", Some("Z:\\replay\\svc"), Kind::ToolStart { call: Call::new("t1", "Bash", json!({ "command": "ls" })) }), 2_100);
    // An event with no folder of its own: its session's.
    hub.publish(&ev(Agent::Claude, "a", None, Kind::Working), 2_200);
    let got = drain(&mut all);
    assert_eq!(got.iter().map(|(_, d)| d["seq"].as_u64().unwrap()).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!((got[0].0.as_str(), got[0].1["type"].as_str(), got[0].1["at"].as_u64(), got[0].1["prompt"].as_str()), ("agent", Some("turnStart"), Some(2_000), Some("fix it")));
    assert_eq!(got[2].1["cwd"], "Z:\\replay\\web");
    let web = drain(&mut web);
    assert_eq!(web.iter().map(|(_, d)| d["seq"].as_u64().unwrap()).collect::<Vec<_>>(), [1, 3]);
    let codex = drain(&mut codex);
    assert_eq!((codex.len(), codex[0].1["call"]["tool"].as_str()), (1, Some("Bash")));
}

#[test]
fn a_client_that_comes_back_gets_what_it_missed() {
    let hub = hub();
    for n in 0..5 {
        hub.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::Working), 2_000 + n);
    }
    let got = drain(&mut hub.events("since=3", None).unwrap());
    assert_eq!(got.iter().map(|(_, d)| d["seq"].as_u64().unwrap()).collect::<Vec<_>>(), [4, 5]);
    // By its Last-Event-ID too.
    assert_eq!(drain(&mut hub.events("", Some("4")).unwrap()).len(), 1);
    // Numbers this hub never gave (it started again since): reset first.
    let got = drain(&mut hub.events("since=99", None).unwrap());
    assert_eq!((got.len(), got[0].0.as_str(), got[0].1["seq"].as_u64()), (1, "reset", Some(5)));
    // More than were kept missed: reset, then what there is.
    for n in 0..(KEPT as u64 + 10) {
        hub.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::Working), 3_000 + n);
    }
    let got = drain(&mut hub.events("since=1", None).unwrap());
    assert_eq!((got[0].0.as_str(), got.len()), ("reset", KEPT + 1));
}

#[test]
fn big_events_are_kept_within_their_bytes() {
    let hub = hub();
    let write = |n: u64, mb: usize| {
        let call = Call::new(&format!("t{n}"), "Write", json!({ "file_path": "Z:\\p\\a.md", "content": "x".repeat(mb * 1024 * 1024) }));
        hub.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::ToolStart { call }), 2_000 + n)
    };
    for n in 1..=4 {
        write(n, 3);
    }
    // The newest that fit in KEPT_BYTES kept; those before told as lost.
    let got = drain(&mut hub.events("since=0", None).unwrap());
    assert_eq!(got.iter().map(|(e, d)| (e.as_str(), d["seq"].as_u64())).collect::<Vec<_>>(), [("reset", Some(4)), ("agent", Some(3)), ("agent", Some(4))]);
    // One bigger than all of it is kept still, alone.
    write(5, 9);
    assert_eq!(drain(&mut hub.events("since=4", None).unwrap()).iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>(), ["agent"]);
}

#[test]
fn a_client_that_reads_nothing_is_let_go() {
    let hub = hub();
    let slow = hub.events("", None).unwrap();
    let mut quick = hub.events("", None).unwrap();
    for n in 0..(ROOM as u64 + 5) {
        hub.publish(&ev(Agent::Claude, "a", None, Kind::Working), n);
        drain(&mut quick);
    }
    assert_eq!(hub.listening(), 1);
    drop(slow);
    // One gone: let go at the next event.
    drop(quick);
    hub.publish(&ev(Agent::Claude, "a", None, Kind::Working), 999);
    assert_eq!(hub.listening(), 0);
}

#[test]
fn the_sessions_known_and_where_to_take_on() {
    let hub = hub();
    hub.publish(&ev(Agent::Claude, "a", Some("Z:\\replay\\web"), Kind::TurnStart { prompt: None }), 2_000);
    hub.publish(&ev(Agent::Dsh, "d", Some("Z:\\replay\\ws"), Kind::Done), 2_100);
    let all = hub.sessions("").unwrap();
    assert_eq!((all["seq"].as_u64(), all["sessions"].as_array().unwrap().len(), all["boot"].as_u64()), (Some(2), 2, Some(1_000)));
    let web = hub.sessions("cwd=z:/replay/WEB/").unwrap();
    assert_eq!((web["sessions"].as_array().unwrap().len(), web["sessions"][0]["id"].as_str(), web["sessions"][0]["status"].as_str()), (1, Some("a"), Some("working")));
    assert_eq!(hub.sessions("agent=dsh").unwrap()["sessions"][0]["status"], "done");
    assert_eq!(hub.sessions("agent=claude").unwrap()["sessions"][0]["id"], "a");
    // A folder only begins with the one asked for: not one of its own.
    assert_eq!(hub.sessions("cwd=Z:\\replay\\we").unwrap()["sessions"].as_array().unwrap().len(), 0);
    let info = hub.info("wakuwaku", "0.3.4");
    assert_eq!((info["api"].as_u64(), info["seq"].as_u64(), info["app"].as_str(), info["capabilities"].clone()), (Some(1), Some(2), Some("wakuwaku"), json!(["sessions", "events", "proof"])));
}

#[test]
fn work_given_up_is_told() {
    let hub = hub();
    hub.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::TurnStart { prompt: None }), 2_000);
    let mut stream = hub.events("cwd=Z:\\elsewhere", None).unwrap();
    assert!(!hub.tick(2_000 + agent_core::registry::STALE_MS));
    assert!(hub.tick(2_001 + agent_core::registry::STALE_MS));
    assert_eq!(hub.sessions("").unwrap()["sessions"][0]["status"], "idle");
    // To every subscriber: a client re-reads its own sessions.
    let got = drain(&mut stream);
    assert_eq!((got.len(), got[0].0.as_str(), got[0].1["seq"].as_u64()), (1, "sessions", Some(1)));
}

// A client that comes back to another start of the hub's (its app started
// again: its numbers began anew) is told so by its Last-Event-ID, however
// many events the new start has sent (zhoupengjie/agent-hub#3).
#[test]
fn a_client_from_another_start_is_told() {
    let before = Hub::with_token("t".repeat(32), PathBuf::from("Z:\\nowhere\\agent-hub\\token"), 1_000);
    for n in 0..5 {
        before.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::Working), 2_000 + n);
    }
    let got = blocks(&mut before.events("since=4", None).unwrap());
    assert_eq!((got[0].0.as_str(), got[0].2["seq"].as_u64()), ("1000:5", Some(5)));
    let after = Hub::with_token("t".repeat(32), PathBuf::from("Z:\\nowhere\\agent-hub\\token"), 9_000);
    for n in 0..8 {
        after.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::Working), 10_000 + n);
    }
    // Its id from the start before: reset, and none of this start's taken for its own.
    for got in [drain(&mut after.events("", Some("1000:5")).unwrap()), drain(&mut after.events("since=1000%3A5", None).unwrap())] {
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0.as_str(), got[0].1["boot"].as_u64(), got[0].1["seq"].as_u64()), ("reset", Some(9_000), Some(8)));
    }
    // This start's own id, or a number alone (as before): those after it.
    for id in ["9000:5", "5"] {
        let got = drain(&mut after.events("", Some(id)).unwrap());
        assert_eq!(got.iter().map(|(e, d)| (e.as_str(), d["seq"].as_u64().unwrap())).collect::<Vec<_>>(), [("agent", 6), ("agent", 7), ("agent", 8)], "{id}");
    }
    // One it cannot read: from now on.
    assert!(drain(&mut after.events("", Some("x:5")).unwrap()).is_empty());
}

// An agent it does not know is passed over; asked for none it knows, an
// error, not every agent's sessions (zhoupengjie/agent-hub#5).
#[test]
fn agents_it_does_not_know() {
    let hub = hub();
    hub.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::Working), 1);
    hub.publish(&ev(Agent::Codex, "c", Some("Z:\\p"), Kind::Working), 2);
    for query in ["agent=codx", "agent=foo,bar"] {
        assert!(hub.sessions(query).unwrap_err().starts_with("no agent "), "{query}");
        assert!(hub.events(query, None).is_err(), "{query}");
    }
    let some = hub.sessions("agent=codex,kimi").unwrap();
    assert_eq!((some["sessions"].as_array().unwrap().len(), some["sessions"][0]["id"].as_str()), (1, Some("c")));
    assert_eq!(hub.sessions("agent=").unwrap()["sessions"].as_array().unwrap().len(), 2);
}

// A turn's end comes with the files it changed (zhoupengjie/agent-hub#6).
#[test]
fn an_ending_says_the_files_changed() {
    let hub = hub();
    let mut stream = hub.events("", None).unwrap();
    hub.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::TurnStart { prompt: None }), 1);
    hub.publish(&ev(Agent::Claude, "a", None, Kind::ToolDone { call: Call { file: Some("Z:\\p\\a.md".into()), ..Call::new("t1", "Edit", json!({})) } }), 2);
    hub.publish(&ev(Agent::Claude, "a", None, Kind::Done), 3);
    hub.publish(&ev(Agent::Claude, "a", None, Kind::TurnStart { prompt: None }), 4);
    hub.publish(&ev(Agent::Claude, "a", None, Kind::Stopped), 5);
    let got = drain(&mut stream);
    assert_eq!(got.iter().map(|(_, d)| d.get("files").cloned()).collect::<Vec<_>>(), [None, None, Some(json!(["Z:\\p\\a.md"])), None, Some(json!([]))]);
}

// Where a client finds the hub: this machine's PORT, unless told otherwise.
#[test]
fn where_a_client_finds_the_hub() {
    std::env::remove_var(URL_VAR);
    assert_eq!(url(), "http://127.0.0.1:47213");
    std::env::set_var(URL_VAR, " http://127.0.0.1:47300/ ");
    assert_eq!(url(), "http://127.0.0.1:47300");
    std::env::remove_var(URL_VAR);
}

#[test]
fn queries_are_read_as_sent() {
    assert_eq!(decode("Z%3A%5Cmy+proj%2Fa"), "Z:\\my proj/a");
    assert_eq!(decode("100%"), "100%");
    assert_eq!(decode("%zz"), "%zz");
    assert_eq!(Filter::from_query("agent=codex,claude&cwd=").unwrap().agents, [Agent::Codex, Agent::Claude]);
}

// HMAC-SHA256 as RFC 4231 has it (its test cases 1, 2 and 6: a key longer
// than the block hashed first).
#[test]
fn hmac_is_as_the_rfc_has_it() {
    let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    assert_eq!(hex(hmac_sha256(&[0x0b; 20], b"Hi There")), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    assert_eq!(hex(hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    assert_eq!(hex(hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
}

// A client reads only a token's file: whatever is on the port may name any
// (zhoupengjie/agent-hub#4).
#[test]
fn a_client_reads_only_a_token() {
    let dir = std::env::temp_dir().join(format!("agent-hub-read-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("keys")).unwrap();
    let good = "0123456789abcdef0123456789abcdef";
    let file = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    assert_eq!(read_token(&file("token", &format!("{good}\r\n"))).as_deref(), Some(good));
    // Not named token, or a token's form in a file of another name.
    assert_eq!(read_token(&file("id_ed25519", good)), None);
    // Not a token's form: upper case, too short, too long, a key's text.
    for text in [good.to_uppercase(), good[..31].to_string(), format!("{good}0"), "-----BEGIN OPENSSH PRIVATE KEY-----".into(), format!("{good}{}", " ".repeat(40))] {
        assert_eq!(read_token(&file("token", &text)), None, "{text}");
    }
    // A relative path, a folder named token, none at all.
    assert_eq!(read_token(Path::new("token")), None);
    std::fs::create_dir_all(dir.join("keys").join("token")).unwrap();
    assert_eq!(read_token(&dir.join("keys").join("token")), None);
    assert_eq!(read_token(&dir.join("gone").join("token")), None);
    let _ = std::fs::remove_dir_all(&dir);
}

// A hub keeps a token of the form and makes anew one that is not, so that a
// client's check never turns it away.
#[test]
fn a_hub_keeps_only_a_token() {
    let dir = std::env::temp_dir().join(format!("agent-hub-keep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let good = "0123456789abcdef0123456789abcdef";
    std::fs::write(dir.join("token"), good).unwrap();
    assert!(Hub::open(&dir, 0).unwrap().allows(Some(&format!("Bearer {good}"))));
    std::fs::write(dir.join("token"), "Z".repeat(40)).unwrap();
    let hub = Hub::open(&dir, 0).unwrap();
    let made = read_token(&dir.join("token")).expect("a token made anew");
    assert!(hub.allows(Some(&format!("Bearer {made}"))) && made != "Z".repeat(32));
    let _ = std::fs::remove_dir_all(&dir);
}

// /v1/info?nonce= proves the hub knows its token; one that does not cannot.
#[test]
fn the_hub_proves_it_knows_the_token() {
    let hub = hub();
    let token = "t".repeat(32);
    let info = hub.info_for("wakuwaku", "0.3.5", "nonce=r4nd0m%2Bn");
    let given = info["proof"].as_str().unwrap();
    assert!(proves(&token, "r4nd0m+n", given));
    assert!(!proves(&"u".repeat(32), "r4nd0m+n", given));
    assert!(!proves(&token, "another", given));
    // None asked, an empty one, one too long: no proof.
    for query in ["", "nonce=", &format!("nonce={}", "n".repeat(NONCE_MAX + 1))] {
        assert!(hub.info_for("wakuwaku", "0.3.5", query).get("proof").is_none(), "{query}");
    }
    assert_eq!(hub.info("wakuwaku", "0.3.5")["hub"], env!("CARGO_PKG_VERSION"));
}
