# agent-hub

[English](README.en.md)

跟踪 coding agent 会话的两个 Rust crate：从 agent 的 hook 事件（和它留下的会话记录、会话日志）读出“这个会话在做什么”，再告诉本机别的应用。[Wakuwaku](https://github.com/zhoupengjie/Wakuwaku)（跟着 agent 干活的桌面宠物和任务栏）用的就是它们。

支持：Claude Code、Codex、Antigravity（agy）、OpenCode、oh-my-pi（omp）、ZCode、Reasonix、DeepSeek Harness。

## 两个 crate

- **agent-core**：每家 agent 的事件读成同一种 `AgentEvent`（`event.rs`：哪家、哪个会话、在做什么），各家的怪癖在各自的模块里（`claude.rs`、`codex.rs`……）。`registry.rs` 从这些事件记下有哪些会话：状态、回合、正在跑的调用、待办、这一轮改过的文件、怎么结束的、迟到的事件、子代理、超时放弃。`asks.rs` 是等人回答的确认请求：面板要显示什么、每个选择回给各家 hook 什么、什么时候算在 agent 自己的窗口里答过。不带窗口、端口，不依赖 Tauri。
- **agent-hub**：把会话告诉本机别的应用（编辑器、脚本）：
  - `GET /v1/info`：API 版本、能给什么、在哪个应用里、最新事件的编号、令牌文件在哪儿（不要令牌）；
  - `GET /v1/sessions`：现在有哪些会话，可以按文件夹（`?cwd=`）、按 agent（`?agent=`，不认识的名字跳过，一个都不认识时回 400）筛选；
  - `GET /v1/events`：Server-Sent Events，每个事件带编号（`seq`；`id:` 是 `启动时间:编号`），一来就发；一轮结束（`done`、`stopped`、`failed`）时带上这一轮改过的文件（`files`）；`?since=` 或 `Last-Event-ID` 补上漏掉的，hub 重启过就先发 `reset`。

  后两个要带本机令牌（`Authorization: Bearer <令牌>`），令牌放在宿主应用自己的数据文件夹里。它不带 HTTP 服务器：宿主应用用自己的服务器接请求，交给它回答（每个接口的细节写在 `agent-hub/src/lib.rs` 开头）。

hook 由宿主应用安装、转发（Wakuwaku 的做法见它的 `docs/DEVELOPMENT.md`）。

## 客户端怎么找到 hub

一台机器同一时间只有一个 hub：占着端口 `47213`（`agent_hub::PORT`）的那个应用；后起来的应用发现端口被占，就当客户端。客户端：

1. 生成一个随机的 nonce（至少 128 位，用系统的随机数，比如 16 个随机字节转成十六进制），问 `GET http://127.0.0.1:47213/v1/info?nonce=<nonce>`（设了环境变量 `AGENT_HUB_URL` 就用它：测试、换端口时；`agent_hub::url()`），确认 `api` 是 1、`capabilities` 里有 `proof`。没有回答，或回答里没有 `api`，就是没有 hub。
2. 用 `agent_hub::read_token(token.file)` 读令牌：只认这台电脑本地盘上的完整路径（网络路径、管道、设备路径只看字符串就拒绝，碰都不碰，免得把登录凭据发给对方）、文件名是 `token`、文件本身和上面每一级目录都不是链接或联接点（hub 报的 `token.file` 是去掉了链接的规范化路径，`\\?\C:\…`；数据目录在挂到文件夹、没有盘符的卷上时，报这个卷的名字开头的路径，`\\?\Volume{…}\…`）、最多 64 字节、内容正好是 32 位小写十六进制，否则当作没有 hub，什么都不发。
3. 用 `agent_hub::proves(令牌, nonce, proof)` 核对回答里的 `proof`（令牌对 nonce 的 HMAC-SHA256），对不上就当作没有 hub。
4. 带着 `Authorization: Bearer <令牌>` 请求 `/v1/sessions`、`/v1/events`。

第 2、3 步是因为 hub 没开时，本机别的程序（包括别的用户账号的）可能先占住这个端口、冒充 hub：它可以让客户端去读任意文件当令牌发给它，或者指向真的令牌文件骗走令牌。hub 发来的会话和事件也只当作要显示的数据，不照着它做任何事。不在别处写任何用来“发现”的文件。

## 用法

```toml
[dependencies]
agent-core = { git = "https://github.com/zhoupengjie/agent-hub", tag = "v0.2.6" }
agent-hub = { git = "https://github.com/zhoupengjie/agent-hub", tag = "v0.2.6" }
```

```rust
use agent_core::{event::Agent, Context};

// 一个 hook 事件（JSON）读成 AgentEvent，记进会话，告诉订阅者。
let ctx = Context { now, dsh_home: &dsh_home };
if let Some(ev) = agent_core::read(Agent::Codex, &mut hook_event, &ctx) {
    hub.publish(&ev, now);
}
```

## AgentEvent

JSON 是 camelCase，事件的种类在 `type` 里；`AgentEvent::from_json` 读回来：

```json
{ "agent": "claude", "session": "…", "cwd": "D:\\work\\notes", "type": "toolStart",
  "call": { "id": "toolu_…", "tool": "Bash", "input": { "command": "npm test" } } }
```

`type` 有 `sessionStart`、`sessionEnd`、`turnStart`、`toolStart`、`toolDone`、`toolFailed`、`waiting`、`working`、`rest`、`stopped`、`done`、`failed`、`taskDone`；工具名和参数一律按 Claude Code 的叫法（agent 自己的叫法在 `own`、`raw` 里）：编辑带着改的内容（`old_string`、`new_string`、`replace_all`，几处一起改的是 `MultiEdit` 的 `edits`），读文件带着行（`offset`、`limit`）。改完的文件是整条路径：`call.file`，补丁改了几个文件都在 `call.files`。宿主应用的插件为了控制大小截断过输入时，`call.truncated` 是 `true`。每个字段的意思写在 `agent-core/src/event.rs`。

## 版本

用 tag 发版（`vX.Y.Z`）。`/v1` 里只会加字段，不兼容的改动换成 `/v2`（`agent_hub::API`）；客户端先看 `/v1/info` 的 `capabilities`。

## 测试

```bash
cargo test --workspace
```

现在只在 Windows 上测（路径在那里比较时不分大小写），以后加 macOS。

## 许可

MIT，见 [LICENSE](LICENSE)。
