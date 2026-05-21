# TelegramMCP

A local Rust MCP server that exposes a Telegram Bot to LLM clients via stdio,
with bidirectional message flow and a searchable local SQLite history.

Open source and transparent — single static binary, audited dependency tree
(see `deny.toml`), no proprietary services in the loop.

## What it does

- **Send.** `tg_send_message`, `tg_send_photo`, `tg_send_document`,
  `tg_edit_message`, `tg_delete_message`, `tg_forward_message`,
  `tg_send_chat_action`.
- **Read.** A background long-poll task captures incoming updates into SQLite.
  Tools: `tg_history_list_chats`, `tg_history_get_chat`, `tg_history_messages`
  (paginated, newest-first), `tg_history_search` (FTS5), `tg_history_get_message`,
  `tg_history_mark_read`, `tg_history_download` (fetch media bytes on demand).
- **Identity.** `tg_bot_whoami`, `tg_bot_list_aliases`.

## Requirements

- Rust stable (toolchain pinned via `rust-toolchain.toml`)
- A Telegram bot token from [@BotFather](https://t.me/BotFather)

## Build

```powershell
cargo build -p mcp-server --release
# Output: target/release/TelegramMCP.exe (Windows) or TelegramMCP (other)
```

## Configure

```powershell
Copy-Item config.example.toml config.toml
notepad config.toml
$env:TELEGRAM_BOT_TOKEN = "123456:your-token-from-botfather"
```

The bot token is **only** sourced from the env var named in `[bot] token_env`
(default `TELEGRAM_BOT_TOKEN`). Inline `[bot] token` is supported but warns.

## Run

```powershell
.\target\release\TelegramMCP.exe --config config.toml
```

Point your MCP host (e.g., Claude Desktop) at the binary. The server speaks
JSON-RPC on stdio.

## Verify and audit

```powershell
cargo deny check          # license + advisory check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

Inspect the history store directly:

```powershell
sqlite3 history.db ".schema"
sqlite3 history.db "SELECT chat_id, message_id, direction, text FROM messages ORDER BY date DESC LIMIT 10;"
```

## Architecture

See [docs/superpowers/specs/2026-05-13-telegram-mcp-design.md](docs/superpowers/specs/2026-05-13-telegram-mcp-design.md).

Workspace crates:
- `aliases` — chat-name resolution
- `history` — SQLite store (schema, migrations, FTS5)
- `tg-client` — outbound Bot API wrapper
- `tg-updater` — background `getUpdates` long-poller
- `local-pipe` — Windows named-pipe IPC (lets local hooks call the server)
- `mcp-server` — `rmcp`-backed binary that wires it all together
- `tg-hook` — Claude Code hook binary: Stop + AskUserQuestion (Windows)

## Security model

- **stdio transport.** Whoever spawned the binary already has access.
- **Bot token.** Never logged. Source from env var. `config.toml` is
  gitignored; `config.example.toml` is the tracked template.
- **No MTProto / user-account API.** Only the Bot API (HTTPS).
- **Allowlists.** Optional `[access] allowed_chats` (inbound drop filter) and
  `allowed_send_targets` (outbound deny) keep blast radius small.
- **File tools.** Optional `[access] file_root` confines `tg_history_download`
  / `tg_send_photo` / `tg_send_document` to one directory; unset leaves them
  unconfined (the MCP client is trusted).
- **Local pipe.** The server also serves its tools over a per-instance Windows
  named pipe for local hooks like `tg-hook` — DACL-restricted to the current
  user, claimed with `first_pipe_instance`, and gated by a per-instance token.

## License

MIT OR Apache-2.0.
