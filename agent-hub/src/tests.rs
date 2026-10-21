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

// A network path, a pipe, a device's: turned away by their text alone,
// nothing opened, nothing sent over a network (192.0.2.1 is an address for
// documents, reaching nothing). A drive's path is this machine's
// (zhoupengjie/agent-hub#4).
#[cfg(windows)]
#[test]
fn a_token_is_read_only_from_this_machine_s_disks() {
    let start = std::time::Instant::now();
    for p in [
        r"\\192.0.2.1\share\token",
        r"\\attacker@80\dav\token",
        r"\\.\pipe\token",
        r"\\?\UNC\192.0.2.1\share\token",
        r"\\?\GLOBALROOT\Device\X\token",
    ] {
        assert!(!local_path(Path::new(p)), "{p}");
        assert_eq!(read_token(Path::new(p)), None, "{p}");
    }
    assert!(start.elapsed() < Duration::from_secs(1), "nothing reached for");
    assert!(local_path(Path::new(r"C:\data\token")) && local_path(Path::new(r"\\?\C:\data\token")));
    assert!(!local_path(Path::new(r"C:token")) && !local_path(Path::new(r"\data\token")));
}

// A volume's path by its name (\\?\Volume{…}\…, a volume's with no drive
// letter) is read; a name of another form there is not.
#[cfg(windows)]
#[test]
fn a_token_is_read_by_its_volume_s_name() {
    let guid = r"\\?\Volume{0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0}";
    assert!(local_path(Path::new(&format!(r"{guid}\data\token"))));
    for p in [
        r"\\.\Volume{0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0}\data\token",
        r"\\?\Volume{0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f}\data\token",
        r"\\?\Volume{0b1c2d3e-4f50-6172-8394-a5b6c7d8e9fg}\data\token",
        r"\\?\Volume{0b1c2d3e4f50-6172-8394-a5b6c7d8e9f00}\data\token",
        r"\\?\Volume0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0\data\token",
        r"\\?\Volume{0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0}x\data\token",
        r"\\?\HarddiskVolume3\data\token",
    ] {
        assert!(!local_path(Path::new(p)), "{p}");
    }
    let dir = std::env::temp_dir().join(format!("agent-hub-volume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("token"), "0123456789abcdef0123456789abcdef").unwrap();
    let by_volume = volume_path(&dir.join("token")).expect("the temp folder's volume has a name");
    let text = by_volume.to_str().unwrap();
    assert!(text.starts_with(r"\\?\Volume{") && text.ends_with(r"\token"), "{text}");
    assert_eq!(read_token(&by_volume).as_deref(), Some("0123456789abcdef0123456789abcdef"));
    let _ = std::fs::remove_dir_all(&dir);
}

// A hub whose folder is on a volume mounted on a folder (no drive letter)
// names its token's file so that a client reads it. Needs such a folder,
// made by an administrator, named in AGENT_HUB_TEST_MOUNTED:
// cargo test -- --ignored a_hub_on_a_mounted_volume
#[cfg(windows)]
#[test]
#[ignore]
fn a_hub_on_a_mounted_volume_names_its_token_s_file_as_a_client_reads_it() {
    let mounted = std::env::var("AGENT_HUB_TEST_MOUNTED").expect("AGENT_HUB_TEST_MOUNTED: a folder on a volume mounted on a folder");
    let dir = Path::new(&mounted).join(format!("agent-hub-mounted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let hub = Hub::open(&dir, 0).unwrap();
    assert_eq!(read_token(hub.token_file()), None, "not through the folder the volume is mounted on");
    let named = hub.info_for("wakuwaku", "0.3.5", "")["token"]["file"].as_str().unwrap().to_string();
    println!("named: {named}");
    let token = read_token(Path::new(&named)).expect("its token, read");
    assert!(hub.allows(Some(&format!("Bearer {token}"))));
    let _ = std::fs::remove_dir_all(&dir);
}

// A link named token is not read, though it points to a real token's file.
// Windows lets a link be made only with the right (an administrator's, or
// developer mode): without it, the real file alone.
#[test]
fn a_link_named_token_is_not_followed() {
    let dir = std::env::temp_dir().join(format!("agent-hub-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("real")).unwrap();
    std::fs::create_dir_all(dir.join("link")).unwrap();
    let (target, link) = (dir.join("real").join("token"), dir.join("link").join("token"));
    std::fs::write(&target, "0123456789abcdef0123456789abcdef").unwrap();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&target, &link).is_ok();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&target, &link).is_ok();
    if made {
        assert_eq!(read_token(&link), None);
    }
    assert_eq!(read_token(&target).as_deref(), Some("0123456789abcdef0123456789abcdef"));
    let _ = std::fs::remove_dir_all(&dir);
}

// A FIFO named token does not keep the client waiting.
#[cfg(unix)]
#[test]
fn a_fifo_named_token_does_not_hang() {
    let dir = std::env::temp_dir().join(format!("agent-hub-fifo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("token");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert_eq!(read_token(&fifo), None);
    let _ = std::fs::remove_dir_all(&dir);
}

// A folder of the token's path that is a junction (any account may make
// one): not read through; the path with it taken out is (zhoupengjie/agent-hub#4).
#[cfg(windows)]
#[test]
fn a_token_under_a_junction_is_not_read_through_it() {
    let dir = std::env::temp_dir().join(format!("agent-hub-junction-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("real")).unwrap();
    std::fs::write(dir.join("real").join("token"), "0123456789abcdef0123456789abcdef").unwrap();
    let link = dir.join("link");
    let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(dir.join("real")).output().unwrap();
    assert!(made.status.success(), "{made:?}");
    assert!(!no_linked_folder(&link.join("token")));
    assert_eq!(read_token(&link.join("token")), None);
    let whole = std::fs::canonicalize(link.join("token")).unwrap();
    assert!(no_linked_folder(&whole));
    assert_eq!(read_token(&whole).as_deref(), Some("0123456789abcdef0123456789abcdef"));
    // A folder not there.
    assert!(!no_linked_folder(&dir.join("gone").join("token")));
    let _ = std::fs::remove_dir_all(&dir);
}

// A folder of the token's path that is a link (Windows lets one be made
// with an administrator's right or in developer mode; without it, skipped):
// not read through.
#[test]
fn a_token_under_a_linked_folder_is_not_read_through_it() {
    let dir = std::env::temp_dir().join(format!("agent-hub-linkdir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("real")).unwrap();
    std::fs::write(dir.join("real").join("token"), "0123456789abcdef0123456789abcdef").unwrap();
    let link = dir.join("link");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(dir.join("real"), &link).is_ok();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(dir.join("real"), &link).is_ok();
    if made {
        assert!(!no_linked_folder(&link.join("token")));
        assert_eq!(read_token(&link.join("token")), None);
    }
    assert!(read_token(&dir.join("real").join("token")).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

// A hub whose folder is reached through a junction tells its token's file
// by the path with the junction taken out, which a client reads.
#[cfg(windows)]
#[test]
fn a_hub_names_its_token_s_file_as_a_client_reads_it() {
    let dir = std::env::temp_dir().join(format!("agent-hub-named-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("real")).unwrap();
    let link = dir.join("link");
    let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(dir.join("real")).output().unwrap();
    assert!(made.status.success(), "{made:?}");
    let hub = Hub::open(&link.join("agent-hub"), 0).unwrap();
    let named = hub.info_for("wakuwaku", "0.3.5", "")["token"]["file"].as_str().unwrap().to_string();
    assert!(named.starts_with(r"\\?\") && !named.contains(r"\link\"), "{named}");
    let token = read_token(Path::new(&named)).expect("its token, read");
    assert!(hub.allows(Some(&format!("Bearer {token}"))));
    let _ = std::fs::remove_dir_all(&dir);
}
