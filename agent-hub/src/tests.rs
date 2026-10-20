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
            (field("event"), serde_json::from_str(&field("data")).unwrap_or(Value::Null))
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
    let mut all = hub.events("", None);
    let mut web = hub.events("cwd=Z%3A%5Creplay%5Cweb", None);
    let mut codex = hub.events("agent=codex", None);
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
    let got = drain(&mut hub.events("since=3", None));
    assert_eq!(got.iter().map(|(_, d)| d["seq"].as_u64().unwrap()).collect::<Vec<_>>(), [4, 5]);
    // By its Last-Event-ID too.
    assert_eq!(drain(&mut hub.events("", Some("4"))).len(), 1);
    // Numbers this hub never gave (it started again since): reset first.
    let got = drain(&mut hub.events("since=99", None));
    assert_eq!((got.len(), got[0].0.as_str(), got[0].1["seq"].as_u64()), (1, "reset", Some(5)));
    // More than were kept missed: reset, then what there is.
    for n in 0..(KEPT as u64 + 10) {
        hub.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::Working), 3_000 + n);
    }
    let got = drain(&mut hub.events("since=1", None));
    assert_eq!((got[0].0.as_str(), got.len()), ("reset", KEPT + 1));
}

#[test]
fn a_client_that_reads_nothing_is_let_go() {
    let hub = hub();
    let slow = hub.events("", None);
    let mut quick = hub.events("", None);
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
    let all = hub.sessions("");
    assert_eq!((all["seq"].as_u64(), all["sessions"].as_array().unwrap().len(), all["boot"].as_u64()), (Some(2), 2, Some(1_000)));
    let web = hub.sessions("cwd=z:/replay/WEB/");
    assert_eq!((web["sessions"].as_array().unwrap().len(), web["sessions"][0]["id"].as_str(), web["sessions"][0]["status"].as_str()), (1, Some("a"), Some("working")));
    assert_eq!(hub.sessions("agent=dsh")["sessions"][0]["status"], "done");
    assert_eq!(hub.sessions("agent=claude")["sessions"][0]["id"], "a");
    // A folder only begins with the one asked for: not one of its own.
    assert_eq!(hub.sessions("cwd=Z:\\replay\\we")["sessions"].as_array().unwrap().len(), 0);
    let info = hub.info("wakuwaku", "0.3.4");
    assert_eq!((info["api"].as_u64(), info["seq"].as_u64(), info["app"].as_str(), info["capabilities"].clone()), (Some(1), Some(2), Some("wakuwaku"), json!(["sessions", "events"])));
}

#[test]
fn work_given_up_is_told() {
    let hub = hub();
    hub.publish(&ev(Agent::Claude, "a", Some("Z:\\p"), Kind::TurnStart { prompt: None }), 2_000);
    let mut stream = hub.events("cwd=Z:\\elsewhere", None);
    assert!(!hub.tick(2_000 + agent_core::registry::STALE_MS));
    assert!(hub.tick(2_001 + agent_core::registry::STALE_MS));
    assert_eq!(hub.sessions("")["sessions"][0]["status"], "idle");
    // To every subscriber: a client re-reads its own sessions.
    let got = drain(&mut stream);
    assert_eq!((got.len(), got[0].0.as_str(), got[0].1["seq"].as_u64()), (1, "sessions", Some(1)));
}

#[test]
fn queries_are_read_as_sent() {
    assert_eq!(decode("Z%3A%5Cmy+proj%2Fa"), "Z:\\my proj/a");
    assert_eq!(decode("100%"), "100%");
    assert_eq!(decode("%zz"), "%zz");
    assert_eq!(Filter::from_query("agent=codex,claude&cwd=").agents, [Agent::Codex, Agent::Claude]);
}
