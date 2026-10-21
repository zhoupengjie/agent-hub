# agent-hub

[中文](README.md)

Two Rust crates for following coding agents' sessions: what a session is doing, read from the agent's hook events (and the transcript and session log it leaves), told to other apps on the same machine. [Wakuwaku](https://github.com/zhoupengjie/Wakuwaku), a desktop pet and taskbar that follows your agents at work, is built on them.

Supported: Claude Code, Codex, Antigravity (agy), OpenCode, oh-my-pi (omp), ZCode, Reasonix, DeepSeek Harness.

## The two crates

- **agent-core**: every agent's events read into one kind of `AgentEvent` (`event.rs`: whose, which session, what it is doing), each agent's quirks in its own module (`claude.rs`, `codex.rs`, ...). `registry.rs` keeps the sessions known from those events: status, turn, the calls running, the to-do list, the files the turn changed, how it ended, late events, subagents, work given up. `asks.rs` is the permission prompts and questions waiting on the person: what a panel shows, the reply each agent's hook takes for each choice, when one was answered in the agent's own window. No window, no port, no Tauri.
- **agent-hub**: the sessions told to other apps on the machine (an editor, a script):
  - `GET /v1/info`: the API version, what it tells, the app it runs in, the latest event's number, where the token is (no token needed);
  - `GET /v1/sessions`: the sessions known, by folder (`?cwd=`) or agent (`?agent=`; a name it does not know is passed over, none it knows is a 400);
  - `GET /v1/events`: Server-Sent Events, each event numbered (`seq`; its `id:` is `boot:seq`), sent as it comes; a turn's end (`done`, `stopped`, `failed`) with the files the turn changed (`files`); `?since=` or `Last-Event-ID` sends what was missed first, a `reset` first when the hub started again since.

  The last two take a local token (`Authorization: Bearer <token>`), kept in the host app's own data folder. It holds no HTTP server: the host app serves the requests with its own and hands them to it (each endpoint is described at the top of `agent-hub/src/lib.rs`).

The hooks are installed and forwarded by the host app (Wakuwaku's way: its `docs/DEVELOPMENT.md`).

## How a client finds the hub

One hub on a machine at a time: the app that holds port `47213` (`agent_hub::PORT`); an app that finds it taken is a client. A client:

1. makes a random nonce (128 bits at least, from the system's randomness: 16 random bytes in hex, say) and asks `GET http://127.0.0.1:47213/v1/info?nonce=<nonce>` (or at `AGENT_HUB_URL` when that is set: a test, another port; `agent_hub::url()`), and checks `api` is 1 and `capabilities` has `proof`. No answer, or one with no `api`: no hub.
2. reads the token with `agent_hub::read_token(token.file)`: only a whole path on this machine's own disks (a network path, a pipe or a device's is turned away by its text, never opened: opening one would send the person's sign-in to it), a file named `token` that is no link itself, under folders none of which is a link or junction (the hub names its `token.file` by the path with those taken out, `\\?\C:\…`), 64 bytes at most, holding 32 lowercase hex digits; anything else is no hub, and nothing is sent.
3. checks the answer's `proof` (HMAC-SHA256 of the nonce under the token) with `agent_hub::proves(token, nonce, proof)`; one that does not match is no hub.
4. asks `/v1/sessions` and `/v1/events` with `Authorization: Bearer <token>`.

Steps 2 and 3 because while no hub runs, another program on the machine (another user's too) may take the port and pose as one: to have the client read any file and send it as the token, or name the real token's file to have the token. What the hub tells is data to show, never what to do. Nothing is written anywhere else to find it by.

## Use

```toml
[dependencies]
agent-core = { git = "https://github.com/zhoupengjie/agent-hub", tag = "v0.2.3" }
agent-hub = { git = "https://github.com/zhoupengjie/agent-hub", tag = "v0.2.3" }
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

`type` is one of `sessionStart`, `sessionEnd`, `turnStart`, `toolStart`, `toolDone`, `toolFailed`, `waiting`, `working`, `rest`, `stopped`, `done`, `failed`, `taskDone`; tools and their input go by Claude Code's names (the agent's own in `own` and `raw`): an edit with its text (`old_string`, `new_string`, `replace_all`; several at once a `MultiEdit`'s `edits`), a read with its lines (`offset`, `limit`). A finished edit's file is its whole path, `call.file`, and every file a patch changed is in `call.files`. Where the host app's plugin cut an input short to keep an event small, `call.truncated` is `true`. Every field is described in `agent-core/src/event.rs`.

## Versions

Released by tags (`vX.Y.Z`). `/v1` only ever gains fields; a change that breaks it is `/v2` (`agent_hub::API`), and a client looks at `/v1/info`'s `capabilities` first.

## Tests

```bash
cargo test --workspace
```

Tested on Windows for now (paths are compared there without case); macOS to come.

## License

MIT, see [LICENSE](LICENSE).
