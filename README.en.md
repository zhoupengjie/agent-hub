# agent-hub

[中文](README.md)

Two Rust crates for following coding agents' sessions: what a session is doing, read from the agent's hook events (and the transcript and session log it leaves), told to other apps on the same machine. [Wakuwaku](https://github.com/zhoupengjie/Wakuwaku), a desktop pet and taskbar that follows your agents at work, is built on them.

Supported: Claude Code, Codex, Antigravity (agy), OpenCode, oh-my-pi (omp), ZCode, Reasonix, DeepSeek Harness.

## The two crates

- **agent-core**: every agent's events read into one kind of `AgentEvent` (`event.rs`: whose, which session, what it is doing), each agent's quirks in its own module (`claude.rs`, `codex.rs`, ...). `registry.rs` keeps the sessions known from those events: status, turn, the calls running, the to-do list, the files the turn changed, how it ended, late events, subagents, work given up. `asks.rs` is the permission prompts and questions waiting on the person: what a panel shows, the reply each agent's hook takes for each choice, when one was answered in the agent's own window. No window, no port, no Tauri.
- **agent-hub**: the sessions told to other apps on the machine (an editor, a script):
  - `GET /v1/info`: the API version, what it tells, the app it runs in, the latest event's number, where the token is (no token needed);
  - `GET /v1/sessions`: the sessions known, by folder (`?cwd=`) or agent (`?agent=`);
  - `GET /v1/events`: Server-Sent Events, each event numbered, sent as it comes; `?since=` or `Last-Event-ID` sends what was missed first.

  The last two take a local token (`Authorization: Bearer <token>`), kept in the host app's own data folder. It holds no HTTP server: the host app serves the requests with its own and hands them to it (each endpoint is described at the top of `agent-hub/src/lib.rs`).

The hooks are installed and forwarded by the host app (Wakuwaku's way: its `docs/DEVELOPMENT.md`).

## Use

```toml
[dependencies]
agent-core = { git = "https://github.com/zhoupengjie/agent-hub", tag = "v0.1.0" }
agent-hub = { git = "https://github.com/zhoupengjie/agent-hub", tag = "v0.1.0" }
```

```rust
use agent_core::{event::Agent, Context};

// A hook's event (JSON) read as an AgentEvent, kept, and told to the subscribers.
let ctx = Context { now, dsh_home: &dsh_home };
if let Some(ev) = agent_core::read(Agent::Codex, &mut hook_event, &ctx) {
    hub.publish(&ev, now);
}
```

## AgentEvent

As JSON, camelCase, its kind in `type`; `AgentEvent::from_json` reads one back:

```json
{ "agent": "claude", "session": "…", "cwd": "D:\\work\\notes", "type": "toolStart",
  "call": { "id": "toolu_…", "tool": "Bash", "input": { "command": "npm test" } } }
```

`type` is one of `sessionStart`, `sessionEnd`, `turnStart`, `toolStart`, `toolDone`, `toolFailed`, `waiting`, `working`, `rest`, `stopped`, `done`, `failed`, `taskDone`; tools and their input go by Claude Code's names (the agent's own in `own` and `raw`). Every field is described in `agent-core/src/event.rs`.

## Versions

Released by tags (`vX.Y.Z`). `/v1` only ever gains fields; a change that breaks it is `/v2` (`agent_hub::API`), and a client looks at `/v1/info`'s `capabilities` first.

## Tests

```bash
cargo test --workspace
```

Tested on Windows for now (paths are compared there without case); macOS to come.

## License

MIT, see [LICENSE](LICENSE).
