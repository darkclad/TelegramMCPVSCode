# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Project

TelegramMCP is a Rust MCP server that exposes a Telegram Bot to LLM clients
over stdio. Bidirectional: outbound Bot API calls + a background long-poll
loop that captures incoming updates into a local SQLite history. Design in
[docs/superpowers/specs/2026-05-13-telegram-mcp-design.md](docs/superpowers/specs/2026-05-13-telegram-mcp-design.md);
implementation plan in [docs/superpowers/plans/2026-05-13-telegram-mcp.md](docs/superpowers/plans/2026-05-13-telegram-mcp.md).

Binaries:
- `TelegramMCP` — the MCP server. JSON-RPC over stdio. Also listens on
  a per-instance Windows named pipe (`local-pipe`) so local processes
  like `tg-hook` can call its tools without spawning a second server.
- `tg-hook` — Claude Code hook (Windows). Auto-detects its mode from the
  `hook_event_name` in stdin:
  - **Stop** — sends a wakeup + the last response to Telegram, blocks
    polling history for the user's reply, and returns it to Claude as
    `{"decision":"block","reason":"..."}`. On timeout, returns a
    configurable retry-message so Claude takes another turn.
  - **PreToolUse** (matcher `AskUserQuestion`) — relays the question's
    options to Telegram and blocks the tool (exit code 2) with the user's
    reply, so multiple-choice prompts can be answered from Telegram.

  Ctrl+C, local keyboard input, or the timeout releases the hook and lets
  the in-app flow proceed normally.

### Wiring tg-hook into Claude Code

Add to `~/.claude/settings.json` (or the workspace's `.claude/settings.json`):

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "D:\\Programs\\user\\TelegramMCP\\tg-hook.exe --chat me --message \"Claude finished. Reply on Telegram to continue.\" --retry-message \"Still waiting on you.\" --timeout-secs 3600 --poll-secs 5",
            "timeout": 3700
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "AskUserQuestion",
        "hooks": [
          {
            "type": "command",
            "command": "D:\\Programs\\user\\TelegramMCP\\tg-hook.exe --chat me --timeout-secs 3600 --poll-secs 5",
            "timeout": 3700
          }
        ]
      }
    ]
  }
}
```

The `Stop` hook bridges finished turns; the `PreToolUse` hook (matcher
`AskUserQuestion`) surfaces multiple-choice prompts — reply on Telegram with
the option number(s). Both entries run the same binary, which picks its mode
from the `hook_event_name` in stdin. `--message` is omitted from the
`PreToolUse` entry: that mode builds its message from the question itself.

`timeout` (seconds) must be larger than the hook's `--timeout-secs` or
Claude Code will kill the hook before it can return.

## Common commands

```powershell
cargo build                                        # debug build, all crates
cargo check --workspace                            # fastest feedback loop
cargo build -p mcp-server --release                # release server only

cargo test --workspace --all-targets               # everything
cargo test -p history --test messages              # one integration test file
cargo test -p mcp-server --test smoke              # e2e via fake Bot API

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

cargo run -p mcp-server -- --config config.toml    # run the server
```

On Windows, run plain `cargo test --workspace` (not `--all-targets`) from a
non-elevated shell: any test binary whose name contains `update` trips UAC
installer-detection (error 740). Integration tests all run; only the empty
`tg-updater` lib harness is skipped (it has `[lib] test = false`).

The toolchain in `rust-toolchain.toml` is stable. Workspace lints in
`Cargo.toml` enable `clippy::pedantic` everywhere — expect to satisfy
`uninlined_format_args`, `similar_names`, etc. Allow inline only with a
one-line `// reason` comment.

## Architecture

The workspace splits along *capability boundaries*, not domain layers. Each
crate owns one externally-visible concern:

- **mcp-server** — binary. Wires `rmcp` to a tool registry. Reads `config.toml`,
  constructs per-feature contexts (`TgClient`, `History`, `Aliases`,
  `allowed_send_targets`), spawns the updater task, dispatches `tools/call`.
  Tool I/O types live in `tools_io.rs`.
- **tg-client** — outbound only. Typed wrapper around `teloxide::Bot`.
  Stateless. Owns `TgClientError`.
- **tg-updater** — the only place the long-poll loop lives. Owns
  `getUpdates`, offset persistence, backoff, inbound access filtering.
  Pushes `StoredMessage`s into `history`. Owns `UpdaterError`.
- **history** — SQLite store. Owns schema, migrations, FTS5 index, all
  read/write APIs. Owns `HistoryError`.
- **aliases** — chat-name resolution. `Aliases::resolve(&ChatRef) -> Result<i64, UnknownAlias>`.
- **local-pipe** — Windows named-pipe IPC. Lets local processes (e.g.
  `tg-hook`) call a running server's tools without spawning a second one.
  Owns the per-instance pipe (hardened DACL + `first_pipe_instance`), the
  `AUTH <token>` handshake, and the discovery file. Owns `PipeError`.
- **tg-hook** — binary. The Claude Code Stop hook (see *Binaries* above).
  `anyhow` throughout — it is a binary, and its `lib.rs` exists only so the
  integration tests can drive its modules.

### Critical conventions

- **stdout is the MCP transport.** Server crates must never `println!`. Use
  `tracing` (logs to stderr).
- **`rmcp` is pinned to 0.2** with `server` + `transport-io` features.
- **`thiserror` per crate, `anyhow` only at the binary boundary.**
- **Capability-per-crate.** A new domain (e.g., webhook transport) becomes a
  new crate, not a module in `mcp-server`.
- **All outbound chats resolved via `Aliases::resolve` first.** Numeric
  passes through, string lookups go through the configured `[aliases]` map.
  Unknown alias is a hard error, never silent fallback.
- **No real Telegram in CI.** All HTTP tests use `wiremock` with
  `[bot] api_base_url` pointed at the mock server.

### Tool registration

In `crates/mcp-server/src/main.rs`: each tool is declared via
`tool(name, description, schema_obj::<InputType>())`, then dispatched in a
single `match` on the tool name inside `ServerHandler::call_tool`. Input/output
types implement `serde::Deserialize` + `schemars::JsonSchema` so the input
schema is generated automatically. To add a tool: define I/O types in
`tools_io.rs`, register it in `tools()`, add a match arm.

### Tests

- Crate-local unit tests live next to source.
- Integration tests per crate at `crates/<name>/tests/<topic>.rs`.
- The end-to-end smoke test `crates/mcp-server/tests/smoke.rs` spawns the
  real binary, drives a full MCP handshake over stdio, and asserts on tool
  responses. This is the canonical e2e shape; mirror its `McpClient` helper
  (`tests/common/mod.rs`) when adding new e2e tests.
