# TelegramMCP — Design

**Status:** approved (brainstorming complete, 2026-05-13)
**Owner:** user
**Sibling reference:** [FileSystemMCP](../../../../FileSystem/) — same workspace conventions, lints, and `rmcp` version.

## One-line

A local Rust MCP server that exposes a Telegram **Bot API** account as tools to an LLM, with bidirectional message flow and a searchable local history.

## Goals & non-goals

### Goals
- **Open source and transparent.** Single static binary, auditable dependency tree, no proprietary services in the loop.
- **Bidirectional.** LLM can both send messages and read incoming traffic the bot received.
- **History.** Every message the bot sees or sends is persisted locally and queryable by the LLM (list, paginate, full-text search).
- **Friendly chat targeting.** LLM addresses chats by alias (`"alerts"`) or raw `chat_id`.
- **Conservative blast radius.** Inbound and outbound allowlists; no auto-download of media.
- **Consistency with FileSystemMCP.** Same `rmcp` version, same crate layout pattern, same lint posture, same logging discipline.

### Non-goals (v1)
- MTProto / user-account API (no phone-number auth, no session files).
- Webhook transport. Long-polling only.
- Multi-bot. One bot per server process.
- Inline keyboards, callback queries, polls, payments, Telegram Web Apps.
- Group administration (ban / restrict / promote).
- Auto-download of media. (On-demand only, via `tg_history_download`.)
- Edit-history tracking. Latest text wins; previous revisions are not retained.

## Use cases driving the design

1. **Notification / alert sink.** LLM (or scripts the LLM writes) sends notifications and status into chats and channels. Mostly outbound with occasional ack reads.
2. **LLM as Telegram correspondent.** User talks to the bot; LLM reads incoming messages, drafts replies, sends them back through the same MCP.

Use cases explicitly *not* driving v1: group archive / search-everything, general Bot API admin surface.

## Language, frameworks, dependencies

| Concern | Choice | Rationale |
|---|---|---|
| Language | Rust (stable, pinned via `rust-toolchain.toml`) | Consistency with FileSystemMCP; single static binary for "transparent" distribution. |
| MCP | `rmcp` 0.2, features `server` + `transport-io` | Pinned to match sibling project. |
| Async runtime | `tokio` (full features) | Required by `teloxide` and `rmcp`. |
| Telegram Bot API | `teloxide` | Mature dispatcher-based Bot API client; we use its `Bot` HTTP client and `getUpdates` long-poller. |
| Storage | `rusqlite` | Same crate as FileSystemMCP's `MemoryStore`. WAL mode. |
| JSON | `serde` + `serde_json` | Standard. |
| Schemas | `schemars` | Auto-generates tool input schemas from Rust types (matches sibling). |
| Errors | `thiserror` per crate; `anyhow` only at the binary boundary | Same rule as sibling. |
| Logging | `tracing` to stderr | **stdout is the MCP transport — no `println!` in server crates.** |
| HTTP test double | `wiremock` | For unit/integration tests without hitting `api.telegram.org`. |
| Lints | `clippy::pedantic` workspace-wide | Same as sibling. Allow inline only with a one-line `// reason` comment. |
| Supply chain | `cargo deny` config (`deny.toml`) | License allowlist, ban unmaintained crates. |

## Architecture

Workspace split by capability boundary, mirroring FileSystemMCP:

```
TelegramMCP/
├── Cargo.toml                  (workspace, pedantic lints)
├── rust-toolchain.toml         (stable, pinned)
├── clippy.toml
├── rustfmt.toml
├── deny.toml
├── config.example.toml
├── crates/
│   ├── mcp-server/             # binary: TelegramMCP.exe — JSON-RPC over stdio
│   ├── tg-client/              # outbound: typed wrapper around teloxide's Bot
│   ├── tg-updater/             # inbound: long-poll task writing into history
│   ├── history/                # SQLite store: chats, messages, FTS search
│   ├── aliases/                # chat-name aliases (config + resolution)
│   └── mcp-console/            # optional REPL client (matches sibling project)
├── docs/
└── tests/
```

### Crate responsibilities

- **`mcp-server`** — binary `TelegramMCP`. Wires `rmcp` to a tool registry. Reads `config.toml`, constructs per-feature contexts (`Bot`, `History`, `Aliases`, `AccessPolicy`), spawns the updater task, dispatches `tools/call` to the appropriate crate. Tool I/O types live in `tools_io.rs`. No business logic.
- **`tg-client`** — outbound only. Typed wrapper around `teloxide::Bot`. Stateless. Knows nothing about history. Owns `TgClientError`.
- **`tg-updater`** — the *only* place the long-poll loop lives. Owns the `getUpdates` task, offset persistence, backoff, and inbound access filtering. Pushes `StoredMessage` into `history`. Owns `UpdaterError`.
- **`history`** — SQLite store. Owns the schema, migrations, FTS5 index, all read/write APIs. Owns `HistoryError`. Same architectural role as `safety` + `fs-index` in the sibling project.
- **`aliases`** — chat-name resolution. `resolve(chat: &ChatRef) -> Result<i64, UnknownAlias>`. Loaded from config at startup. Cheap and self-contained.
- **`mcp-console`** — optional REPL client crate (`TelegramMCP-console` binary). Matches sibling project's pattern; raw JSON-RPC over the child's stdio.

### Critical conventions (inherited from FileSystemMCP)

- **stdout is the MCP transport.** Server crates must never `println!`. Use `tracing` (logs to stderr).
- **`rmcp` is pinned to 0.2** with `server` + `transport-io` features.
- **`thiserror` per crate, `anyhow` only at the binary boundary.**
- **Capability-per-crate.** A new domain (e.g., webhook transport) becomes a new crate, not a module in `mcp-server`.

## Tool surface

All tools follow `tg_<domain>_<verb>` naming.

### Outbound — `tg_send_*`
Each returns `{ chat_id: i64, message_id: i64, date: i64 }`.

| Tool | Inputs |
|---|---|
| `tg_send_message` | `chat`, `text`, `parse_mode?`, `reply_to?`, `silent?`, `link_preview?` |
| `tg_send_photo` | `chat`, `path \| url`, `caption?`, `parse_mode?` |
| `tg_send_document` | `chat`, `path`, `caption?`, `filename?` |
| `tg_edit_message` | `chat`, `message_id`, `text`, `parse_mode?` |
| `tg_delete_message` | `chat`, `message_id` |
| `tg_forward_message` | `from_chat`, `message_id`, `to_chat` |
| `tg_send_chat_action` | `chat`, `action` (`typing`, `upload_photo`, …) |

### History — `tg_history_*` (read from local SQLite, never re-fetches)

| Tool | Inputs | Returns |
|---|---|---|
| `tg_history_list_chats` | — | array of `{ chat_id, kind, title, username?, last_seen, unread_count }` |
| `tg_history_get_chat` | `chat` | chat metadata |
| `tg_history_messages` | `chat`, `before_message_id?`, `after_message_id?`, `limit` | paginated messages, newest-first; `before`/`after` are message-id cursors, exclusive |
| `tg_history_search` | `query`, `chat?`, `since?`, `until?` | FTS5 results with snippets; `since`/`until` are unix timestamps |
| `tg_history_get_message` | `chat`, `message_id` | single message |
| `tg_history_mark_read` | `chat`, `up_to_message_id` | sets local `unread_count` baseline |
| `tg_history_download` | `chat`, `message_id`, `dest_path` | downloads media bytes via `file_id` |

### Bot identity — `tg_bot_*`

| Tool | Inputs | Returns |
|---|---|---|
| `tg_bot_whoami` | — | username, name, description |
| `tg_bot_list_aliases` | — | configured `alias → chat_id` map |

### Chat targeting

Every tool argument named `chat` (or `from_chat` / `to_chat`) accepts either:
- a numeric `chat_id` (Telegram's native form, may be negative), or
- a string alias defined in `[aliases]`.

Unknown alias → tool error (`UnknownAlias`), never a silent fallback.

### Media handling

When the updater sees an incoming photo / voice / document / video / animation, it stores Telegram's `file_id` + metadata (size, mime, filename, duration) in `messages.media_file_id` / `messages.media_meta`. It **does not** download the bytes. The LLM downloads on demand via `tg_history_download`.

## Data flow

Two independent flows, one shared store.

```
                ┌─────────────────────────┐
   stdin ─────▶ │     mcp-server          │ ◀──── tools/call
                │   (rmcp dispatch)       │
                └────┬────────────────┬───┘
                     │                │
       tg_send_*     │                │   tg_history_*
                     ▼                ▼
              ┌───────────┐    ┌───────────┐
              │ tg-client │    │  history  │
              │ (HTTPS)   │    │ (SQLite)  │
              └─────┬─────┘    └─────▲─────┘
                    │                │
            api.telegram.org         │ writes
                    ▲                │
                    │          ┌─────┴──────┐
                    └──────────┤ tg-updater │  (background tokio task)
                       getUpdates loop      │
                               └────────────┘
```

### Outbound (LLM → Telegram)
1. MCP host calls `tg_send_message`.
2. `mcp-server` resolves alias via `aliases`, applies `allowed_send_targets` policy, validates inputs.
3. `tg-client` calls Bot API (`teloxide::Bot::send_message`).
4. Returned `Message` is normalized and written to `history` with `direction='out'`, so the LLM can see its own sent messages later.
5. Tool returns `{ chat_id, message_id, date }`.

### Inbound (Telegram → store)
1. `tg-updater` is `tokio::spawn`'d at server startup.
2. Loops: `getUpdates(offset, timeout=poll_timeout_secs, allowed_updates=…)`. Telegram holds the connection until new updates or timeout.
3. Updates from chats outside `allowed_chats` (if set) are dropped before they reach the store.
4. For each surviving update: upsert `chats` row, insert `messages` row (`direction='in'`), update FTS index. The raw update JSON is stored in `messages.raw` for forward-compat and debugging.
5. Persist new `offset` to `kv` *after* the batch commits, so restarts don't replay or skip.
6. Failure → exponential backoff (1s → 30s cap), keep retrying. Log via `tracing`. The MCP server never exits on updater failure; it logs and keeps reading history.

## Storage

SQLite via `rusqlite`. WAL mode (`PRAGMA journal_mode=WAL`) for one-writer / many-readers.

```sql
chats(
  chat_id      INTEGER PRIMARY KEY,    -- Telegram's chat id (can be negative)
  kind         TEXT NOT NULL,          -- 'private' | 'group' | 'supergroup' | 'channel'
  title        TEXT,                   -- group/channel title OR user display name
  username     TEXT,
  first_seen   INTEGER NOT NULL,       -- unix seconds
  last_seen    INTEGER NOT NULL
);

messages(
  chat_id       INTEGER NOT NULL,
  message_id    INTEGER NOT NULL,
  date          INTEGER NOT NULL,       -- unix seconds
  from_id       INTEGER,                -- null for channel posts
  from_name     TEXT,
  reply_to      INTEGER,                -- message_id replied to, null if not a reply
  text          TEXT,                   -- caption for media; null for service messages
  media_kind    TEXT,                   -- 'photo' | 'document' | 'voice' | 'video' | 'animation' | 'audio' | 'sticker'
  media_file_id TEXT,                   -- Telegram file_id, downloadable on demand
  media_meta    TEXT,                   -- json: size, mime, filename, duration, …
  direction     TEXT NOT NULL,          -- 'in' | 'out'
  raw           TEXT NOT NULL,          -- full update JSON
  PRIMARY KEY (chat_id, message_id)
);

CREATE INDEX idx_messages_chat_date ON messages(chat_id, date DESC);

CREATE VIRTUAL TABLE messages_fts USING fts5(
  text, content='messages', content_rowid='rowid'
);

kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- known keys: 'schema_version', 'update_offset', 'last_unread_baseline:<chat_id>'
```

### Concurrency
- Single writer (the updater task + outbound write-after-send path) coordinates via `tokio::sync::Mutex<Connection>` for writes.
- Readers (tool calls) use short-lived connections from a small pool (`r2d2_sqlite` or a hand-rolled pool of 2–4 connections — pick whichever the sibling's `MemoryStore` uses for consistency).

### Migrations
- `kv['schema_version']` tracks the current version.
- `history::open(path)` runs all migrations from current → latest in a single transaction.
- v1 ships at `schema_version = 1`.

### Retention (default: unbounded)
Configurable cap via `[retention]` config block:
- `max_age_days` — drops messages older than N days.
- `max_messages_total` — drops oldest beyond N messages.

A background sweep task runs the trim every 6 hours when retention is configured. Off by default — explicit choice so "transparent" includes "doesn't silently forget."

## Configuration

TOML, path passed via `--config`. Same shape as FileSystemMCP's config.

```toml
[bot]
token_env = "TELEGRAM_BOT_TOKEN"     # preferred
# token = "123456:ABC..."             # alternative; warning logged if used
# api_base_url = "https://api.telegram.org"   # override for tests

[storage]
path = "C:/Users/user/AppData/Local/TelegramMCP/history.db"

[updater]
enabled = true
poll_timeout_secs = 30               # bounded to [1, 50]
allowed_update_kinds = [
  "message", "edited_message",
  "channel_post", "edited_channel_post",
]

[retention]
# max_age_days = 365
# max_messages_total = 1_000_000

[aliases]
me        = 12345678
alerts    = -1001234567890
"team-eng" = -1009876543210

[access]
allowed_chats         = ["me", "alerts", "team-eng"]   # inbound filter; empty = open
allowed_send_targets  = []                              # outbound filter; empty = unrestricted
```

### Startup validation (hard-fail with a clear message)
1. Bot token resolvable (`token_env` set in environment, or `token` inline).
2. `storage.path` parent directory exists and is writable.
3. Every alias referenced in `allowed_chats` / `allowed_send_targets` is defined in `[aliases]`.
4. `poll_timeout_secs ∈ [1, 50]` (Telegram's documented bound).

### Secret hygiene
- `config.example.toml` is the only tracked template.
- `.gitignore` excludes `config.toml`, `history.db`, `*.db-wal`, `*.db-shm`.
- Bot token is never logged. `tg-client` redacts the token in any `Debug` impl. Spans tag requests with bot id only.

## Auth model

- **Telegram side:** the bot token is the only secret. No phone, no session, no 2FA. Same model as any Bot API client.
- **MCP side:** none. Standard MCP-over-stdio: whoever spawned the server already has access. Trust boundary is the MCP host (e.g., Claude Desktop). Identical to FileSystemMCP.

## Access control

Two independent allowlists, both accepting aliases or numeric ids:

| List | Direction | Effect when set | Default |
|---|---|---|---|
| `allowed_chats` | inbound | updates from chats outside the list are dropped before reaching the store | empty (open) |
| `allowed_send_targets` | outbound | `tg_send_*` with a target outside the list returns `ChatNotAllowed` *before* hitting the API | empty (unrestricted) |

## Errors

`thiserror` enum per crate; `anyhow` only at the binary boundary.

```rust
// tg-client
pub enum TgClientError {
    Http(reqwest::Error),
    Api { code: i32, description: String },
    RateLimited { retry_after_secs: u32 },
    ChatNotAllowed(String),
    UnknownAlias(String),
    InvalidChat(String),
}

// history
pub enum HistoryError {
    Sqlite(rusqlite::Error),
    Migration { from: u32, to: u32, source: rusqlite::Error },
    NotFound { chat_id: i64, message_id: i64 },
    Corruption(String),
}

// updater
pub enum UpdaterError {
    Client(TgClientError),
    Store(HistoryError),
    Decode(serde_json::Error),
}
```

- `mcp-server` maps each crate error to an MCP tool-call error response — **never panics** across the JSON-RPC boundary.
- `RateLimited { retry_after_secs }` is surfaced verbatim so the LLM can choose to back off.
- Errors include a stable `code` field in the MCP response (e.g., `"chat_not_allowed"`, `"unknown_alias"`, `"rate_limited"`) for programmatic handling.

## Logging

- Single `tracing` subscriber, format = `tracing_subscriber::fmt`, writer = `stderr`.
- Levels:
  - `info` — lifecycle (`server_started`, `updater_started`, `update_batch_received { count }`, `tool_call { name }`).
  - `warn` — non-fatal Bot API errors, rate limits, dropped updates from disallowed chats.
  - `error` — persistent failures (sqlite open failure, repeated 5xx after backoff cap).
  - `debug` — per-request detail; **only level at which message bodies appear**.
- Token redacted in every span.
- Default level: `info`. Overridable via `RUST_LOG`.

## Testing

| Layer | Tooling | Scope |
|---|---|---|
| Unit | `cargo test` per crate | `aliases` resolution; `history` migrations / FTS / pagination; `tg-client` error mapping (with `wiremock`). |
| Integration — updater | `wiremock` | Scripted `getUpdates` batches. Asserts: offset advances, messages land, idempotency on duplicate update id, backoff on simulated 5xx. |
| End-to-end smoke | `crates/mcp-server/tests/smoke.rs`, modelled on the sibling project's smoke test | Spawn `TelegramMCP.exe`, drive a full MCP handshake over stdio, point at `wiremock`-backed fake Bot API via `[bot] api_base_url`. Assert tool round-trips. |
| Manual / real bot | `tests/manual/` with `#[ignore]` | Documents how to run against a real bot for local verification. **Not in CI.** |

No real Telegram in CI.

## Distribution

- Build: `cargo build -p mcp-server --release` → `target/release/TelegramMCP.exe` (Windows) / `TelegramMCP` (other platforms).
- Static binary; no runtime dependencies on the host.
- `deny.toml` enforces a license allowlist and bans unmaintained crates — same as sibling.
- README documents:
  - How to obtain a bot token from `@BotFather`.
  - How to verify the binary against source (build steps, `cargo deny check`, optional reproducible-build notes).
  - How to inspect the SQLite store directly (`sqlite3 history.db`).
- The `deploy-to-programs` skill is compatible: `d:/Programs/user/TelegramMCP/TelegramMCP.exe` is the target install path, and Claude Desktop's `claude_desktop_config.json` can be repointed there.

## Open questions / future work (not v1)

- Multi-bot in one server process.
- Webhook transport (would land as a new `tg-webhook` crate parallel to `tg-updater`).
- MTProto / user-account API (would land as `tg-user-client` crate; auth flow is a separate design).
- Edit-history table (`message_revisions`) if "latest text wins" turns out to be insufficient.
- Inline keyboards / callback queries — needs flow-state design.
- Reproducible builds and signed releases.
