# TelegramMCP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust MCP server that exposes a Telegram Bot to LLM clients over stdio, with bidirectional message flow and a searchable local SQLite history.

**Architecture:** Cargo workspace with one capability crate per responsibility (`aliases`, `history`, `tg-client`, `tg-updater`, `mcp-server`). The server binary wires `rmcp` to a tool registry, spawns a background long-poll updater that writes incoming Telegram updates into SQLite, and serves outbound Bot API calls via tools.

**Tech Stack:** Rust (stable), `rmcp` 0.2 (stdio transport), `teloxide` (Bot API client), `tokio`, `rusqlite` (SQLite + FTS5), `serde`/`schemars` (tool schemas), `thiserror`/`anyhow` (errors), `tracing` (stderr logging), `wiremock` (HTTP test double).

**Spec:** [../specs/2026-05-13-telegram-mcp-design.md](../specs/2026-05-13-telegram-mcp-design.md)

**Sibling reference for conventions:** `d:\Work\Programming\MCP\FileSystem\` — workspace lints, `rmcp` registration pattern, error→`McpError` mapping, smoke-test harness.

---

## File structure produced by this plan

```
TelegramMCP/
├── Cargo.toml                           workspace + central deps + pedantic lints
├── rust-toolchain.toml                  stable, minimal profile
├── clippy.toml
├── rustfmt.toml
├── deny.toml                            license allowlist
├── config.example.toml                  tracked template
├── README.md
├── CLAUDE.md
├── .gitignore                           (already present)
├── docs/
│   └── superpowers/specs/2026-05-13-telegram-mcp-design.md   (already present)
└── crates/
    ├── aliases/
    │   ├── Cargo.toml
    │   ├── src/lib.rs                   ChatRef, Aliases, UnknownAlias
    │   └── tests/resolve.rs
    ├── history/
    │   ├── Cargo.toml
    │   ├── src/
    │   │   ├── lib.rs                   re-exports
    │   │   ├── types.rs                 StoredMessage, ChatInfo, MediaInfo, Direction
    │   │   ├── error.rs                 HistoryError
    │   │   ├── schema.rs                migrations
    │   │   └── store.rs                 History struct, all queries
    │   └── tests/
    │       ├── migrations.rs
    │       ├── messages.rs
    │       ├── search.rs
    │       └── retention.rs
    ├── tg-client/
    │   ├── Cargo.toml
    │   ├── src/
    │   │   ├── lib.rs
    │   │   ├── error.rs                 TgClientError
    │   │   ├── client.rs                TgClient struct + Bot wrapper
    │   │   └── types.rs                 SendMessageInput etc. shared with server tool I/O
    │   └── tests/
    │       ├── send.rs
    │       ├── updates.rs
    │       └── download.rs
    ├── tg-updater/
    │   ├── Cargo.toml
    │   ├── src/
    │   │   ├── lib.rs
    │   │   ├── error.rs                 UpdaterError
    │   │   ├── mapping.rs               Update → StoredMessage
    │   │   └── loop.rs                  Updater task
    │   └── tests/loop.rs
    └── mcp-server/
        ├── Cargo.toml
        ├── src/
        │   ├── main.rs                  rmcp wiring, dispatch, lifecycle
        │   ├── config.rs                Config struct, validation
        │   ├── tools_io.rs              all tool input/output types
        │   └── error.rs                 unified error → McpError mapping
        └── tests/smoke.rs               end-to-end via wiremock-backed Bot API
```

Test files at `tests/<name>.rs` are integration tests (each compiles to a separate binary, has its own `main`-style entry).

---

## Milestone 1 — Workspace skeleton

### Task 1: Create the Cargo workspace and lint posture

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `clippy.toml`
- Create: `rustfmt.toml`
- Create: `deny.toml`

- [ ] **Step 1: Write `rust-toolchain.toml`**

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

- [ ] **Step 2: Write workspace `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = [
    "crates/aliases",
    "crates/history",
    "crates/tg-client",
    "crates/tg-updater",
    "crates/mcp-server",
]

[workspace.package]
version = "0.0.1"
edition = "2024"
license = "MIT OR Apache-2.0"
authors = ["TelegramMCP authors"]
rust-version = "1.85"
repository = "https://example.invalid/TelegramMCP"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"
missing_docs = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
must_use_candidate = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"

[workspace.dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }

# MCP
rmcp = { version = "0.2", features = ["server", "transport-io"] }

# Telegram Bot API
teloxide = { version = "0.13", default-features = false, features = ["native-tls", "ctrlc_handler"] }

# Storage
rusqlite = { version = "0.32", features = ["bundled"] }

# Serde / schemas
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }
schemars = { version = "0.8", features = ["preserve_order"] }

# Errors
thiserror = "1"
anyhow = "1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Config
toml = "0.8"

# URL / HTTP
url = "2"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }

# Test infrastructure
wiremock = "0.6"
tempfile = "3"

# Internal crates
aliases = { path = "crates/aliases" }
history = { path = "crates/history" }
tg-client = { path = "crates/tg-client" }
tg-updater = { path = "crates/tg-updater" }

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
```

- [ ] **Step 3: Write `clippy.toml`**

```toml
# Pedantic threshold for cognitive complexity. The sibling MCP uses defaults; mirror.
```

(Empty body is fine — file presence signals "use repo-wide clippy config".)

- [ ] **Step 4: Write `rustfmt.toml`**

```toml
edition = "2024"
max_width = 100
use_field_init_shorthand = true
```

- [ ] **Step 5: Write `deny.toml`**

```toml
[licenses]
version = 2
confidence-threshold = 0.95
allow = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-DFS-2016",
    "Unicode-3.0",
    "Zlib",
    "MPL-2.0",
    "CC0-1.0",
]

[bans]
multiple-versions = "warn"
wildcards = "deny"

[advisories]
version = 2
ignore = []

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

- [ ] **Step 6: Verify workspace parses**

Run: `cargo metadata --no-deps --format-version=1 > NUL`
Expected: exits 0. (No crates exist yet so output is sparse — that's fine.)

Actually, since no member crates exist yet, `cargo metadata` will fail. Skip the verify until Task 2 lands the first crate.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml rust-toolchain.toml clippy.toml rustfmt.toml deny.toml
git commit -m "chore: workspace skeleton, lints, deny config"
```

---

## Milestone 2 — `aliases` crate

### Task 2: aliases crate skeleton

**Files:**
- Create: `crates/aliases/Cargo.toml`
- Create: `crates/aliases/src/lib.rs`

- [ ] **Step 1: Write `crates/aliases/Cargo.toml`**

```toml
[package]
name = "aliases"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 2: Write stub `crates/aliases/src/lib.rs`**

```rust
//! Chat-name alias resolution.
//!
//! Loaded from the `[aliases]` block of `config.toml`. The server resolves
//! every `chat` tool argument through [`Aliases::resolve`] before issuing
//! a Bot API call.

#![allow(missing_docs)] // filled in by later tasks
```

- [ ] **Step 3: Verify it builds**

Run: `cargo check -p aliases`
Expected: PASS (no errors, no warnings).

- [ ] **Step 4: Commit**

```bash
git add crates/aliases/
git commit -m "feat(aliases): empty crate skeleton"
```

### Task 3: ChatRef type and Aliases::resolve

**Files:**
- Modify: `crates/aliases/src/lib.rs`
- Create: `crates/aliases/tests/resolve.rs`

- [ ] **Step 1: Write the failing test at `crates/aliases/tests/resolve.rs`**

```rust
use aliases::{Aliases, ChatRef, UnknownAlias};
use std::collections::BTreeMap;

fn fixture() -> Aliases {
    let mut m = BTreeMap::new();
    m.insert("alerts".to_string(), -1001234567890_i64);
    m.insert("me".to_string(), 12345678_i64);
    Aliases::new(m)
}

#[test]
fn numeric_passes_through() {
    let a = fixture();
    let id = a.resolve(&ChatRef::Id(42)).unwrap();
    assert_eq!(id, 42);
}

#[test]
fn known_alias_resolves() {
    let a = fixture();
    let id = a.resolve(&ChatRef::Name("alerts".into())).unwrap();
    assert_eq!(id, -1001234567890);
}

#[test]
fn unknown_alias_errors() {
    let a = fixture();
    let err = a.resolve(&ChatRef::Name("nope".into())).unwrap_err();
    assert!(matches!(err, UnknownAlias { name } if name == "nope"));
}

#[test]
fn names_lists_aliases_sorted() {
    let a = fixture();
    let names: Vec<&str> = a.names().collect();
    assert_eq!(names, vec!["alerts", "me"]);
}
```

- [ ] **Step 2: Run the test, confirm it fails**

Run: `cargo test -p aliases --test resolve`
Expected: FAIL with errors about `Aliases`, `ChatRef`, `UnknownAlias` not found.

- [ ] **Step 3: Implement in `crates/aliases/src/lib.rs`**

```rust
//! Chat-name alias resolution.

use serde::Deserialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// Caller-supplied reference to a Telegram chat: either the raw numeric id
/// or a name that resolves through configured aliases.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ChatRef {
    Id(i64),
    Name(String),
}

/// Configured alias table. Construct with [`Aliases::new`]; the server
/// loads one from the `[aliases]` config block at startup.
#[derive(Debug, Clone, Default)]
pub struct Aliases {
    map: BTreeMap<String, i64>,
}

impl Aliases {
    #[must_use]
    pub fn new(map: BTreeMap<String, i64>) -> Self {
        Self { map }
    }

    pub fn resolve(&self, r: &ChatRef) -> Result<i64, UnknownAlias> {
        match r {
            ChatRef::Id(id) => Ok(*id),
            ChatRef::Name(n) => self
                .map
                .get(n)
                .copied()
                .ok_or_else(|| UnknownAlias { name: n.clone() }),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }

    #[must_use]
    pub fn as_map(&self) -> &BTreeMap<String, i64> {
        &self.map
    }
}

#[derive(Debug, Error)]
#[error("unknown chat alias: {name}")]
pub struct UnknownAlias {
    pub name: String,
}
```

Then remove the `#![allow(missing_docs)]` line if you added it.

- [ ] **Step 4: Run the test, confirm it passes**

Run: `cargo test -p aliases --test resolve`
Expected: 4 passed.

- [ ] **Step 5: Lint check**

Run: `cargo clippy -p aliases --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/aliases/
git commit -m "feat(aliases): ChatRef, Aliases::resolve, UnknownAlias error"
```

---

## Milestone 3 — `history` crate

### Task 4: history crate skeleton and types

**Files:**
- Create: `crates/history/Cargo.toml`
- Create: `crates/history/src/lib.rs`
- Create: `crates/history/src/types.rs`
- Create: `crates/history/src/error.rs`

- [ ] **Step 1: Write `crates/history/Cargo.toml`**

```toml
[package]
name = "history"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
rusqlite = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Write `crates/history/src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("schema migration from v{from} to v{to} failed: {source}")]
    Migration { from: u32, to: u32, source: rusqlite::Error },
    #[error("message {chat_id}/{message_id} not found")]
    NotFound { chat_id: i64, message_id: i64 },
    #[error("stored data corruption: {0}")]
    Corruption(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("blocking task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}
```

- [ ] **Step 3: Write `crates/history/src/types.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction { In, Out }

impl Direction {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
    pub(crate) fn from_sql(s: &str) -> Option<Self> {
        match s {
            "in" => Some(Direction::In),
            "out" => Some(Direction::Out),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatKind { Private, Group, Supergroup, Channel }

impl ChatKind {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            ChatKind::Private => "private",
            ChatKind::Group => "group",
            ChatKind::Supergroup => "supergroup",
            ChatKind::Channel => "channel",
        }
    }
    pub(crate) fn from_sql(s: &str) -> Option<Self> {
        match s {
            "private" => Some(ChatKind::Private),
            "group" => Some(ChatKind::Group),
            "supergroup" => Some(ChatKind::Supergroup),
            "channel" => Some(ChatKind::Channel),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatInfo {
    pub chat_id: i64,
    pub kind: ChatKind,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_seen: i64,
    pub last_seen: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSummary {
    #[serde(flatten)]
    pub info: ChatInfo,
    pub unread_count: i64,
    pub last_message_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub chat_id: i64,
    pub message_id: i64,
    pub date: i64,
    pub from_id: Option<i64>,
    pub from_name: Option<String>,
    pub reply_to: Option<i64>,
    pub text: Option<String>,
    pub media_kind: Option<String>,
    pub media_file_id: Option<String>,
    pub media_meta: Option<serde_json::Value>,
    pub direction: Direction,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub chat_id: i64,
    pub message_id: i64,
    pub date: i64,
    pub snippet: String,
}
```

- [ ] **Step 4: Write `crates/history/src/lib.rs`**

```rust
//! Local SQLite-backed history store for Telegram messages.

pub mod error;
mod schema;
mod store;
pub mod types;

pub use error::HistoryError;
pub use store::History;
pub use types::{ChatInfo, ChatKind, ChatSummary, Direction, SearchHit, StoredMessage};
```

- [ ] **Step 5: Stub `crates/history/src/schema.rs` and `crates/history/src/store.rs`**

```rust
// crates/history/src/schema.rs
use crate::HistoryError;
use rusqlite::Connection;

pub(crate) const CURRENT_VERSION: u32 = 1;

pub(crate) fn migrate(_conn: &mut Connection) -> Result<u32, HistoryError> {
    unimplemented!("filled by Task 5")
}
```

```rust
// crates/history/src/store.rs
use crate::HistoryError;
use std::path::Path;

#[derive(Debug)]
pub struct History;

impl History {
    pub fn open(_path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        unimplemented!("filled by Task 5")
    }
}
```

- [ ] **Step 6: Verify it builds**

Run: `cargo check -p history`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/history/
git commit -m "feat(history): crate skeleton, types, error enum"
```

### Task 5: Schema migrations and `History::open`

**Files:**
- Modify: `crates/history/src/schema.rs`
- Modify: `crates/history/src/store.rs`
- Create: `crates/history/tests/migrations.rs`

- [ ] **Step 1: Write the failing test at `crates/history/tests/migrations.rs`**

```rust
use history::History;
use tempfile::tempdir;

#[test]
fn open_creates_db_and_schema_version_is_set() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("h.db");
    let _h = History::open(&db).unwrap();
    assert!(db.exists());
    // Open again — should be idempotent (no migration re-runs)
    let _h2 = History::open(&db).unwrap();
}

#[test]
fn schema_version_reads_back_as_one() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("h.db");
    let h = History::open(&db).unwrap();
    assert_eq!(h.schema_version().unwrap(), 1);
}
```

- [ ] **Step 2: Confirm it fails**

Run: `cargo test -p history --test migrations`
Expected: panic in `History::open` (`unimplemented!`).

- [ ] **Step 3: Implement `crates/history/src/schema.rs`**

```rust
use crate::HistoryError;
use rusqlite::{params, Connection};

pub(crate) const CURRENT_VERSION: u32 = 1;

const SCHEMA_V1: &str = r"
CREATE TABLE chats (
    chat_id    INTEGER PRIMARY KEY,
    kind       TEXT NOT NULL,
    title      TEXT,
    username   TEXT,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL
);

CREATE TABLE messages (
    chat_id       INTEGER NOT NULL,
    message_id    INTEGER NOT NULL,
    date          INTEGER NOT NULL,
    from_id       INTEGER,
    from_name     TEXT,
    reply_to      INTEGER,
    text          TEXT,
    media_kind    TEXT,
    media_file_id TEXT,
    media_meta    TEXT,
    direction     TEXT NOT NULL,
    raw           TEXT NOT NULL,
    PRIMARY KEY (chat_id, message_id)
);

CREATE INDEX idx_messages_chat_date ON messages(chat_id, date DESC);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    text,
    content='messages',
    content_rowid='rowid'
);

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

pub(crate) fn migrate(conn: &mut Connection) -> Result<u32, HistoryError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let current = read_version(conn)?;
    if current >= CURRENT_VERSION {
        return Ok(current);
    }

    let tx = conn.transaction()?;
    if current == 0 {
        tx.execute_batch(SCHEMA_V1)
            .map_err(|e| HistoryError::Migration { from: 0, to: 1, source: e })?;
        tx.execute(
            "INSERT OR REPLACE INTO kv(key, value) VALUES ('schema_version', ?1)",
            params!["1"],
        )?;
    }
    tx.commit()?;
    Ok(CURRENT_VERSION)
}

fn read_version(conn: &Connection) -> Result<u32, HistoryError> {
    // If kv doesn't exist yet, we're at v0.
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='kv'",
        [],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(0);
    }
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}
```

- [ ] **Step 4: Implement `crates/history/src/store.rs`**

```rust
use crate::{schema, HistoryError};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct History {
    inner: Arc<Mutex<Connection>>,
}

impl History {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let mut conn = Connection::open(path)?;
        schema::migrate(&mut conn)?;
        Ok(Self { inner: Arc::new(Mutex::new(conn)) })
    }

    pub fn schema_version(&self) -> Result<u32, HistoryError> {
        let guard = self.inner.blocking_lock();
        let v: String = guard.query_row(
            "SELECT value FROM kv WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )?;
        v.parse().map_err(|_| HistoryError::Corruption("schema_version not a number".into()))
    }

    pub(crate) fn conn(&self) -> Arc<Mutex<Connection>> {
        self.inner.clone()
    }
}
```

- [ ] **Step 5: Verify tests pass**

Run: `cargo test -p history --test migrations`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/history/
git commit -m "feat(history): schema v1 + migrations, History::open"
```

### Task 6: Upsert chats and insert messages

**Files:**
- Modify: `crates/history/src/store.rs`
- Create: `crates/history/tests/messages.rs`

- [ ] **Step 1: Write the failing test at `crates/history/tests/messages.rs`**

```rust
use history::{ChatInfo, ChatKind, Direction, History, StoredMessage};
use serde_json::json;
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, History) {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    (dir, h)
}

fn sample_chat() -> ChatInfo {
    ChatInfo {
        chat_id: 100,
        kind: ChatKind::Private,
        title: Some("alice".into()),
        username: Some("alice".into()),
        first_seen: 1_000,
        last_seen: 1_000,
    }
}

fn sample_msg(message_id: i64, text: &str, direction: Direction) -> StoredMessage {
    StoredMessage {
        chat_id: 100,
        message_id,
        date: 1_000 + message_id,
        from_id: Some(100),
        from_name: Some("alice".into()),
        reply_to: None,
        text: Some(text.into()),
        media_kind: None,
        media_file_id: None,
        media_meta: None,
        direction,
        raw: json!({ "message_id": message_id }),
    }
}

#[tokio::test]
async fn upsert_chat_inserts_then_updates_last_seen() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    let got = h.get_chat(100).await.unwrap().unwrap();
    assert_eq!(got.last_seen, 1_000);

    let mut c = sample_chat();
    c.last_seen = 2_000;
    h.upsert_chat(&c).await.unwrap();
    let got2 = h.get_chat(100).await.unwrap().unwrap();
    assert_eq!(got2.last_seen, 2_000);
    // first_seen must NOT be overwritten on update
    assert_eq!(got2.first_seen, 1_000);
}

#[tokio::test]
async fn insert_message_roundtrip() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    h.insert_message(&sample_msg(1, "hello", Direction::In)).await.unwrap();
    let got = h.get_message(100, 1).await.unwrap();
    assert_eq!(got.text.as_deref(), Some("hello"));
    assert_eq!(got.direction, Direction::In);
}

#[tokio::test]
async fn insert_message_idempotent_on_duplicate_pk() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    h.insert_message(&sample_msg(1, "hello", Direction::In)).await.unwrap();
    // Same (chat_id, message_id) but different text — should overwrite
    h.insert_message(&sample_msg(1, "edited", Direction::In)).await.unwrap();
    let got = h.get_message(100, 1).await.unwrap();
    assert_eq!(got.text.as_deref(), Some("edited"));
}

#[tokio::test]
async fn get_message_missing_returns_not_found() {
    let (_d, h) = fresh();
    let err = h.get_message(100, 999).await.unwrap_err();
    assert!(matches!(err, history::HistoryError::NotFound { .. }));
}
```

- [ ] **Step 2: Confirm it fails**

Run: `cargo test -p history --test messages`
Expected: FAIL with method-not-found errors on `upsert_chat`, `get_chat`, `insert_message`, `get_message`.

- [ ] **Step 3: Extend `crates/history/src/store.rs`**

Append to `impl History`:

```rust
    pub async fn upsert_chat(&self, c: &crate::ChatInfo) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        let c = c.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            guard.execute(
                "INSERT INTO chats(chat_id, kind, title, username, first_seen, last_seen) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(chat_id) DO UPDATE SET \
                    kind=excluded.kind, title=excluded.title, username=excluded.username, \
                    last_seen=excluded.last_seen",
                rusqlite::params![
                    c.chat_id, c.kind.as_sql(), c.title, c.username, c.first_seen, c.last_seen
                ],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn get_chat(&self, chat_id: i64) -> Result<Option<crate::ChatInfo>, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let row = guard.query_row(
                "SELECT chat_id, kind, title, username, first_seen, last_seen \
                 FROM chats WHERE chat_id=?1",
                rusqlite::params![chat_id],
                |r| {
                    let kind_s: String = r.get(1)?;
                    Ok(crate::ChatInfo {
                        chat_id: r.get(0)?,
                        kind: crate::ChatKind::from_sql(&kind_s)
                            .ok_or(rusqlite::Error::InvalidQuery)?,
                        title: r.get(2)?,
                        username: r.get(3)?,
                        first_seen: r.get(4)?,
                        last_seen: r.get(5)?,
                    })
                },
            );
            match row {
                Ok(c) => Ok(Some(c)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(HistoryError::Sqlite(e)),
            }
        })
        .await?
    }

    pub async fn insert_message(&self, m: &crate::StoredMessage) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        let m = m.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let media_meta_s = m
                .media_meta
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            let raw_s = serde_json::to_string(&m.raw)?;
            guard.execute(
                "INSERT INTO messages(\
                    chat_id, message_id, date, from_id, from_name, reply_to, \
                    text, media_kind, media_file_id, media_meta, direction, raw) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                 ON CONFLICT(chat_id, message_id) DO UPDATE SET \
                    date=excluded.date, from_id=excluded.from_id, from_name=excluded.from_name, \
                    reply_to=excluded.reply_to, text=excluded.text, \
                    media_kind=excluded.media_kind, media_file_id=excluded.media_file_id, \
                    media_meta=excluded.media_meta, direction=excluded.direction, \
                    raw=excluded.raw",
                rusqlite::params![
                    m.chat_id, m.message_id, m.date, m.from_id, m.from_name, m.reply_to,
                    m.text, m.media_kind, m.media_file_id, media_meta_s,
                    m.direction.as_sql(), raw_s,
                ],
            )?;
            // Maintain FTS contentless link.
            guard.execute(
                "INSERT INTO messages_fts(rowid, text) \
                 SELECT rowid, text FROM messages WHERE chat_id=?1 AND message_id=?2 \
                 ON CONFLICT(rowid) DO UPDATE SET text=excluded.text",
                rusqlite::params![m.chat_id, m.message_id],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn get_message(&self, chat_id: i64, message_id: i64) -> Result<crate::StoredMessage, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            let row = guard.query_row(
                "SELECT date, from_id, from_name, reply_to, text, media_kind, media_file_id, \
                        media_meta, direction, raw \
                 FROM messages WHERE chat_id=?1 AND message_id=?2",
                rusqlite::params![chat_id, message_id],
                |r| {
                    let dir_s: String = r.get(8)?;
                    let media_meta_s: Option<String> = r.get(7)?;
                    let raw_s: String = r.get(9)?;
                    Ok((r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        media_meta_s, dir_s, raw_s))
                },
            );
            match row {
                Ok((date, from_id, from_name, reply_to, text, media_kind, media_file_id,
                    media_meta_s, dir_s, raw_s)) => {
                    let direction = crate::Direction::from_sql(&dir_s)
                        .ok_or_else(|| HistoryError::Corruption(format!("bad direction: {dir_s}")))?;
                    let media_meta = media_meta_s
                        .map(|s| serde_json::from_str(&s))
                        .transpose()?;
                    let raw = serde_json::from_str(&raw_s)?;
                    Ok(crate::StoredMessage {
                        chat_id, message_id, date, from_id, from_name, reply_to, text,
                        media_kind, media_file_id, media_meta, direction, raw,
                    })
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    Err(HistoryError::NotFound { chat_id, message_id })
                }
                Err(e) => Err(HistoryError::Sqlite(e)),
            }
        })
        .await?
    }
```

- [ ] **Step 4: Verify tests pass**

Run: `cargo test -p history --test messages`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/history/
git commit -m "feat(history): upsert_chat, get_chat, insert_message, get_message"
```

### Task 7: Paginated message listing and chat summaries

**Files:**
- Modify: `crates/history/src/store.rs`
- Modify: `crates/history/tests/messages.rs`

- [ ] **Step 1: Append failing tests to `crates/history/tests/messages.rs`**

```rust
#[tokio::test]
async fn messages_paginated_newest_first() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    for i in 1..=5 {
        h.insert_message(&sample_msg(i, &format!("m{i}"), Direction::In)).await.unwrap();
    }
    let page = h.messages(100, None, None, 3).await.unwrap();
    assert_eq!(page.len(), 3);
    // newest-first: 5, 4, 3
    assert_eq!(page.iter().map(|m| m.message_id).collect::<Vec<_>>(), vec![5, 4, 3]);
}

#[tokio::test]
async fn messages_before_cursor_is_exclusive() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    for i in 1..=5 {
        h.insert_message(&sample_msg(i, &format!("m{i}"), Direction::In)).await.unwrap();
    }
    // before_message_id=3 → return ids strictly less than 3: 2, 1
    let page = h.messages(100, Some(3), None, 10).await.unwrap();
    assert_eq!(page.iter().map(|m| m.message_id).collect::<Vec<_>>(), vec![2, 1]);
}

#[tokio::test]
async fn list_chats_summarises_last_seen_and_count() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    for i in 1..=3 {
        h.insert_message(&sample_msg(i, &format!("m{i}"), Direction::In)).await.unwrap();
    }
    let chats = h.list_chats().await.unwrap();
    assert_eq!(chats.len(), 1);
    let c = &chats[0];
    assert_eq!(c.info.chat_id, 100);
    assert_eq!(c.last_message_id, Some(3));
    // No mark_read called → all 3 unread
    assert_eq!(c.unread_count, 3);
}
```

- [ ] **Step 2: Confirm they fail**

Run: `cargo test -p history --test messages`
Expected: FAIL — `messages`, `list_chats` methods not found.

- [ ] **Step 3: Extend `History`**

```rust
    pub async fn messages(
        &self,
        chat_id: i64,
        before_message_id: Option<i64>,
        after_message_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<crate::StoredMessage>, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<_>, HistoryError> {
            let guard = conn.blocking_lock();
            let mut sql = String::from(
                "SELECT message_id, date, from_id, from_name, reply_to, text, \
                        media_kind, media_file_id, media_meta, direction, raw \
                 FROM messages WHERE chat_id = ?1",
            );
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(chat_id)];
            if let Some(before) = before_message_id {
                sql.push_str(&format!(" AND message_id < ?{}", args.len() + 1));
                args.push(Box::new(before));
            }
            if let Some(after) = after_message_id {
                sql.push_str(&format!(" AND message_id > ?{}", args.len() + 1));
                args.push(Box::new(after));
            }
            sql.push_str(&format!(" ORDER BY message_id DESC LIMIT ?{}", args.len() + 1));
            args.push(Box::new(limit));

            let mut stmt = guard.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
            let iter = stmt.query_map(rusqlite::params_from_iter(param_refs), |r| {
                let dir_s: String = r.get(9)?;
                let media_meta_s: Option<String> = r.get(8)?;
                let raw_s: String = r.get(10)?;
                Ok((r.get::<_, i64>(0)?,  // message_id
                    r.get::<_, i64>(1)?,  // date
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    media_meta_s, dir_s, raw_s))
            })?;

            let mut out = Vec::new();
            for row in iter {
                let (mid, date, from_id, from_name, reply_to, text, media_kind,
                     media_file_id, media_meta_s, dir_s, raw_s) = row?;
                let direction = crate::Direction::from_sql(&dir_s)
                    .ok_or_else(|| HistoryError::Corruption(format!("bad direction: {dir_s}")))?;
                let media_meta = media_meta_s.map(|s| serde_json::from_str(&s)).transpose()?;
                let raw = serde_json::from_str(&raw_s)?;
                out.push(crate::StoredMessage {
                    chat_id, message_id: mid, date, from_id, from_name, reply_to, text,
                    media_kind, media_file_id, media_meta, direction, raw,
                });
            }
            Ok(out)
        })
        .await?
    }

    pub async fn list_chats(&self) -> Result<Vec<crate::ChatSummary>, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<_>, HistoryError> {
            let guard = conn.blocking_lock();
            let mut stmt = guard.prepare(
                "SELECT c.chat_id, c.kind, c.title, c.username, c.first_seen, c.last_seen, \
                        (SELECT MAX(message_id) FROM messages m WHERE m.chat_id=c.chat_id) AS last_msg \
                 FROM chats c \
                 ORDER BY c.last_seen DESC",
            )?;
            let mut chats: Vec<(crate::ChatInfo, Option<i64>)> = stmt
                .query_map([], |r| {
                    let kind_s: String = r.get(1)?;
                    Ok((
                        crate::ChatInfo {
                            chat_id: r.get(0)?,
                            kind: crate::ChatKind::from_sql(&kind_s)
                                .ok_or(rusqlite::Error::InvalidQuery)?,
                            title: r.get(2)?,
                            username: r.get(3)?,
                            first_seen: r.get(4)?,
                            last_seen: r.get(5)?,
                        },
                        r.get::<_, Option<i64>>(6)?,
                    ))
                })?
                .collect::<Result<_, _>>()?;

            let mut out = Vec::with_capacity(chats.len());
            for (info, last_message_id) in chats.drain(..) {
                let unread_baseline: Option<i64> = guard
                    .query_row(
                        "SELECT value FROM kv WHERE key = ?1",
                        rusqlite::params![format!("last_unread_baseline:{}", info.chat_id)],
                        |r| r.get::<_, String>(0).map(|s| s.parse().unwrap_or(0)),
                    )
                    .ok();
                let unread_count: i64 = guard.query_row(
                    "SELECT COUNT(*) FROM messages \
                     WHERE chat_id=?1 AND direction='in' AND message_id > ?2",
                    rusqlite::params![info.chat_id, unread_baseline.unwrap_or(0)],
                    |r| r.get(0),
                )?;
                out.push(crate::ChatSummary { info, unread_count, last_message_id });
            }
            Ok(out)
        })
        .await?
    }
```

- [ ] **Step 4: Run tests, confirm pass**

Run: `cargo test -p history --test messages`
Expected: all 7 (4 from Task 6 + 3 new) pass.

- [ ] **Step 5: Commit**

```bash
git add crates/history/
git commit -m "feat(history): messages pagination, list_chats with unread counts"
```

### Task 8: FTS5 search

**Files:**
- Modify: `crates/history/src/store.rs`
- Create: `crates/history/tests/search.rs`

- [ ] **Step 1: Write the failing test**

```rust
use history::{ChatInfo, ChatKind, Direction, History, StoredMessage};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn fts_finds_matching_message() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.upsert_chat(&ChatInfo {
        chat_id: 1, kind: ChatKind::Private, title: None, username: None,
        first_seen: 0, last_seen: 0,
    }).await.unwrap();
    for (i, text) in ["lunch plans", "dinner tonight", "lunch break too"].iter().enumerate() {
        h.insert_message(&StoredMessage {
            chat_id: 1, message_id: (i + 1) as i64, date: i as i64,
            from_id: None, from_name: None, reply_to: None,
            text: Some((*text).into()),
            media_kind: None, media_file_id: None, media_meta: None,
            direction: Direction::In, raw: json!({}),
        }).await.unwrap();
    }
    let hits = h.search("lunch", None, None, None).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.snippet.to_lowercase().contains("lunch")));
}

#[tokio::test]
async fn search_scopes_by_chat_when_set() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    for chat_id in [1, 2] {
        h.upsert_chat(&ChatInfo {
            chat_id, kind: ChatKind::Private, title: None, username: None,
            first_seen: 0, last_seen: 0,
        }).await.unwrap();
        h.insert_message(&StoredMessage {
            chat_id, message_id: 1, date: 0,
            from_id: None, from_name: None, reply_to: None,
            text: Some("shared keyword".into()),
            media_kind: None, media_file_id: None, media_meta: None,
            direction: Direction::In, raw: json!({}),
        }).await.unwrap();
    }
    let hits = h.search("keyword", Some(1), None, None).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chat_id, 1);
}
```

- [ ] **Step 2: Confirm fails**

Run: `cargo test -p history --test search`
Expected: FAIL — `search` not found.

- [ ] **Step 3: Extend `History`**

```rust
    pub async fn search(
        &self,
        query: &str,
        chat_id: Option<i64>,
        since: Option<i64>,
        until: Option<i64>,
    ) -> Result<Vec<crate::SearchHit>, HistoryError> {
        let conn = self.inner.clone();
        let q = query.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<_>, HistoryError> {
            let guard = conn.blocking_lock();
            let mut sql = String::from(
                "SELECT m.chat_id, m.message_id, m.date, \
                        snippet(messages_fts, 0, '[', ']', '…', 10) AS snip \
                 FROM messages_fts \
                 JOIN messages m ON m.rowid = messages_fts.rowid \
                 WHERE messages_fts MATCH ?1",
            );
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(q)];
            if let Some(c) = chat_id {
                sql.push_str(&format!(" AND m.chat_id = ?{}", args.len() + 1));
                args.push(Box::new(c));
            }
            if let Some(s) = since {
                sql.push_str(&format!(" AND m.date >= ?{}", args.len() + 1));
                args.push(Box::new(s));
            }
            if let Some(u) = until {
                sql.push_str(&format!(" AND m.date <= ?{}", args.len() + 1));
                args.push(Box::new(u));
            }
            sql.push_str(" ORDER BY m.date DESC LIMIT 100");

            let mut stmt = guard.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
            let hits = stmt
                .query_map(rusqlite::params_from_iter(param_refs), |r| {
                    Ok(crate::SearchHit {
                        chat_id: r.get(0)?,
                        message_id: r.get(1)?,
                        date: r.get(2)?,
                        snippet: r.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(hits)
        })
        .await?
    }
```

- [ ] **Step 4: Run, confirm pass**

Run: `cargo test -p history --test search`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/history/
git commit -m "feat(history): FTS5-backed search with chat and time filters"
```

### Task 9: KV store, mark_read, and retention sweep

**Files:**
- Modify: `crates/history/src/store.rs`
- Create: `crates/history/tests/retention.rs`

- [ ] **Step 1: Write the failing test**

```rust
use history::{ChatInfo, ChatKind, Direction, History, StoredMessage};
use serde_json::json;
use tempfile::tempdir;

fn chat(id: i64) -> ChatInfo {
    ChatInfo { chat_id: id, kind: ChatKind::Private, title: None, username: None,
               first_seen: 0, last_seen: 0 }
}

fn msg(chat_id: i64, mid: i64, date: i64) -> StoredMessage {
    StoredMessage {
        chat_id, message_id: mid, date,
        from_id: None, from_name: None, reply_to: None,
        text: Some(format!("m{mid}")),
        media_kind: None, media_file_id: None, media_meta: None,
        direction: Direction::In, raw: json!({}),
    }
}

#[tokio::test]
async fn kv_roundtrip() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.kv_put("update_offset", "42").await.unwrap();
    assert_eq!(h.kv_get("update_offset").await.unwrap().as_deref(), Some("42"));
    assert_eq!(h.kv_get("missing").await.unwrap(), None);
}

#[tokio::test]
async fn mark_read_lowers_unread_count() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.upsert_chat(&chat(1)).await.unwrap();
    for i in 1..=5 {
        h.insert_message(&msg(1, i, i)).await.unwrap();
    }
    let before = &h.list_chats().await.unwrap()[0];
    assert_eq!(before.unread_count, 5);

    h.mark_read(1, 3).await.unwrap();
    let after = &h.list_chats().await.unwrap()[0];
    assert_eq!(after.unread_count, 2); // messages 4 and 5
}

#[tokio::test]
async fn retention_by_age_trims_old_messages() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.upsert_chat(&chat(1)).await.unwrap();
    // dates in seconds: 1 (old) and 1_000_000 (recent)
    h.insert_message(&msg(1, 1, 1)).await.unwrap();
    h.insert_message(&msg(1, 2, 1_000_000)).await.unwrap();
    // keep everything dated >= 999_000
    let removed = h.trim_older_than(999_000).await.unwrap();
    assert_eq!(removed, 1);
    let remaining = h.messages(1, None, None, 100).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].message_id, 2);
}

#[tokio::test]
async fn retention_by_count_keeps_newest_n_per_chat() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.upsert_chat(&chat(1)).await.unwrap();
    for i in 1..=10 {
        h.insert_message(&msg(1, i, i)).await.unwrap();
    }
    let removed = h.trim_per_chat_to(3).await.unwrap();
    assert_eq!(removed, 7);
    let remaining = h.messages(1, None, None, 100).await.unwrap();
    let ids: Vec<i64> = remaining.iter().map(|m| m.message_id).collect();
    assert_eq!(ids, vec![10, 9, 8]);
}
```

- [ ] **Step 2: Confirm fails**

Run: `cargo test -p history --test retention`
Expected: methods not found.

- [ ] **Step 3: Extend `History`**

```rust
    pub async fn kv_put(&self, key: &str, value: &str) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        let key = key.to_string();
        let value = value.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), HistoryError> {
            let guard = conn.blocking_lock();
            guard.execute(
                "INSERT INTO kv(key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![key, value],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn kv_get(&self, key: &str) -> Result<Option<String>, HistoryError> {
        let conn = self.inner.clone();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<String>, HistoryError> {
            let guard = conn.blocking_lock();
            let row: Result<String, _> = guard.query_row(
                "SELECT value FROM kv WHERE key=?1",
                rusqlite::params![key],
                |r| r.get(0),
            );
            match row {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(HistoryError::Sqlite(e)),
            }
        })
        .await?
    }

    pub async fn mark_read(&self, chat_id: i64, up_to_message_id: i64) -> Result<(), HistoryError> {
        self.kv_put(
            &format!("last_unread_baseline:{chat_id}"),
            &up_to_message_id.to_string(),
        )
        .await
    }

    pub async fn trim_older_than(&self, cutoff_unix_secs: i64) -> Result<usize, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, HistoryError> {
            let guard = conn.blocking_lock();
            let removed = guard.execute(
                "DELETE FROM messages WHERE date < ?1",
                rusqlite::params![cutoff_unix_secs],
            )?;
            guard.execute(
                "INSERT INTO messages_fts(messages_fts) VALUES('rebuild')",
                [],
            )?;
            Ok(removed)
        })
        .await?
    }

    pub async fn trim_per_chat_to(&self, keep_newest: i64) -> Result<usize, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, HistoryError> {
            let guard = conn.blocking_lock();
            let removed = guard.execute(
                "DELETE FROM messages WHERE rowid IN ( \
                    SELECT rowid FROM ( \
                        SELECT rowid, ROW_NUMBER() OVER ( \
                            PARTITION BY chat_id ORDER BY message_id DESC \
                        ) AS rn FROM messages \
                    ) WHERE rn > ?1 \
                 )",
                rusqlite::params![keep_newest],
            )?;
            guard.execute(
                "INSERT INTO messages_fts(messages_fts) VALUES('rebuild')",
                [],
            )?;
            Ok(removed)
        })
        .await?
    }
```

- [ ] **Step 4: Run, confirm pass**

Run: `cargo test -p history --test retention`
Expected: 4 passed.

- [ ] **Step 5: Workspace test sweep**

Run: `cargo test -p history`
Expected: all history tests pass.

- [ ] **Step 6: Lint**

Run: `cargo clippy -p history --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/history/
git commit -m "feat(history): kv store, mark_read, retention sweeps"
```

---

## Milestone 4 — `tg-client` crate

### Task 10: tg-client crate skeleton and error type

**Files:**
- Create: `crates/tg-client/Cargo.toml`
- Create: `crates/tg-client/src/lib.rs`
- Create: `crates/tg-client/src/error.rs`

- [ ] **Step 1: Write `crates/tg-client/Cargo.toml`**

```toml
[package]
name = "tg-client"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
teloxide = { workspace = true }
reqwest = { workspace = true }
url = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
wiremock = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 2: Write `crates/tg-client/src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TgClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Telegram API error {code}: {description}")]
    Api { code: i32, description: String },
    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u32 },
    #[error("unknown alias: {0}")]
    UnknownAlias(String),
    #[error("invalid chat reference: {0}")]
    InvalidChat(String),
    #[error("invalid api base URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("teloxide error: {0}")]
    Teloxide(#[from] teloxide::RequestError),
    #[error("file download error: {0}")]
    Download(String),
}

impl TgClientError {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Http(_) | Self::Download(_) => "http_error",
            Self::Api { .. } => "telegram_api_error",
            Self::RateLimited { .. } => "rate_limited",
            Self::UnknownAlias(_) => "unknown_alias",
            Self::InvalidChat(_) => "invalid_chat",
            Self::InvalidUrl(_) => "invalid_url",
            Self::Teloxide(_) => "telegram_api_error",
        }
    }
}
```

- [ ] **Step 3: Write `crates/tg-client/src/lib.rs`**

```rust
//! Outbound Telegram Bot API client.

pub mod error;
mod client;

pub use client::{TgClient, SentMessage};
pub use error::TgClientError;
```

- [ ] **Step 4: Stub `crates/tg-client/src/client.rs`**

```rust
use crate::TgClientError;
use std::fmt;
use url::Url;

#[derive(Clone)]
pub struct TgClient {
    bot: teloxide::Bot,
    api_base: Url,
}

impl fmt::Debug for TgClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TgClient")
            .field("api_base", &self.api_base.as_str())
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct SentMessage {
    pub chat_id: i64,
    pub message_id: i64,
    pub date: i64,
}

impl TgClient {
    pub fn new(token: String, api_base: Option<Url>) -> Result<Self, TgClientError> {
        let url = api_base.clone().unwrap_or_else(|| {
            "https://api.telegram.org".parse().expect("static URL parses")
        });
        let bot = teloxide::Bot::new(token).set_api_url(url.clone());
        Ok(Self { bot, api_base: url })
    }
}
```

- [ ] **Step 5: Verify build**

Run: `cargo check -p tg-client`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tg-client/
git commit -m "feat(tg-client): crate skeleton, error type, TgClient struct"
```

### Task 11: send_message via wiremock

**Files:**
- Modify: `crates/tg-client/src/client.rs`
- Create: `crates/tg-client/tests/send.rs`

- [ ] **Step 1: Write the failing test**

```rust
use tg_client::TgClient;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn send_message_posts_and_parses_response() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();

    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 7,
                "date": 1_700_000_000_i64,
                "chat": { "id": 42, "type": "private" },
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let sent = client.send_message(42, "hi", None, None, false, true).await.unwrap();
    assert_eq!(sent.chat_id, 42);
    assert_eq!(sent.message_id, 7);
    assert_eq!(sent.date, 1_700_000_000);
}

#[tokio::test]
async fn rate_limit_maps_to_rate_limited_error() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();

    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "ok": false,
            "error_code": 429,
            "description": "Too Many Requests: retry after 7",
            "parameters": { "retry_after": 7 }
        })))
        .mount(&server)
        .await;

    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let err = client.send_message(42, "hi", None, None, false, true).await.unwrap_err();
    assert!(matches!(err, tg_client::TgClientError::RateLimited { retry_after_secs: 7 }));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p tg-client --test send`
Expected: FAIL — `send_message` not found.

- [ ] **Step 3: Implement `send_message`**

Append to `impl TgClient` in `crates/tg-client/src/client.rs`:

```rust
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<teloxide::types::ParseMode>,
        reply_to: Option<i64>,
        silent: bool,
        link_preview_enabled: bool,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        use teloxide::types::ReplyParameters;

        let mut req = self.bot.send_message(ChatId(chat_id), text);
        if let Some(pm) = parse_mode {
            req = req.parse_mode(pm);
        }
        if let Some(rid) = reply_to {
            req = req.reply_parameters(ReplyParameters::new(MessageId(rid as i32)));
        }
        if silent {
            req = req.disable_notification(true);
        }
        if !link_preview_enabled {
            req = req.disable_web_page_preview(true);
        }
        let msg = req.await.map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: msg.id.0 as i64,
            date: msg.date.timestamp(),
        })
    }
```

Add at the bottom of `client.rs`:

```rust
fn map_teloxide_err(e: teloxide::RequestError) -> TgClientError {
    use teloxide::RequestError as R;
    match e {
        R::RetryAfter(d) => TgClientError::RateLimited {
            retry_after_secs: d.seconds(),
        },
        R::Api(ref api) => TgClientError::Api {
            code: api_code(api),
            description: api.to_string(),
        },
        other => TgClientError::Teloxide(other),
    }
}

fn api_code(_e: &teloxide::ApiError) -> i32 {
    // teloxide doesn't expose the numeric code on every variant; pick a stable
    // sentinel for non-rate-limit API errors so the LLM has something to match.
    0
}
```

- [ ] **Step 4: Run, confirm pass**

Run: `cargo test -p tg-client --test send`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/tg-client/
git commit -m "feat(tg-client): send_message with rate-limit mapping"
```

### Task 12: send_photo, send_document, edit, delete, forward, chat_action

**Files:**
- Modify: `crates/tg-client/src/client.rs`
- Modify: `crates/tg-client/tests/send.rs`

- [ ] **Step 1: Append failing tests**

```rust
use wiremock::matchers::body_partial_json;
use wiremock::ResponseTemplate;

#[tokio::test]
async fn edit_message_text_round_trip() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/EditMessageText"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 7, "date": 1_700_000_001_i64,
                "chat": { "id": 42, "type": "private" },
                "text": "edited"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let out = client.edit_message_text(42, 7, "edited", None).await.unwrap();
    assert_eq!(out.message_id, 7);
}

#[tokio::test]
async fn delete_message_returns_unit() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/DeleteMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "result": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    client.delete_message(42, 7).await.unwrap();
}

#[tokio::test]
async fn forward_message_returns_new_message_id() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/ForwardMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 11, "date": 1_700_000_002_i64,
                "chat": { "id": 99, "type": "channel" }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let out = client.forward_message(42, 7, 99).await.unwrap();
    assert_eq!(out.chat_id, 99);
    assert_eq!(out.message_id, 11);
}

#[tokio::test]
async fn chat_action_returns_ok() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendChatAction"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "result": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    client.send_chat_action(42, teloxide::types::ChatAction::Typing).await.unwrap();
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p tg-client --test send`
Expected: FAIL — methods not found.

- [ ] **Step 3: Extend `TgClient`**

```rust
    pub async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        parse_mode: Option<teloxide::types::ParseMode>,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        let mut req = self.bot.edit_message_text(
            ChatId(chat_id),
            MessageId(message_id as i32),
            text,
        );
        if let Some(pm) = parse_mode {
            req = req.parse_mode(pm);
        }
        let msg = req.await.map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: msg.id.0 as i64,
            date: msg.date.timestamp(),
        })
    }

    pub async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<(), TgClientError> {
        use teloxide::prelude::*;
        self.bot
            .delete_message(ChatId(chat_id), MessageId(message_id as i32))
            .await
            .map_err(map_teloxide_err)?;
        Ok(())
    }

    pub async fn forward_message(
        &self,
        from_chat: i64,
        message_id: i64,
        to_chat: i64,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        let msg = self
            .bot
            .forward_message(ChatId(to_chat), ChatId(from_chat), MessageId(message_id as i32))
            .await
            .map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: msg.id.0 as i64,
            date: msg.date.timestamp(),
        })
    }

    pub async fn send_chat_action(
        &self,
        chat_id: i64,
        action: teloxide::types::ChatAction,
    ) -> Result<(), TgClientError> {
        use teloxide::prelude::*;
        self.bot.send_chat_action(ChatId(chat_id), action).await.map_err(map_teloxide_err)?;
        Ok(())
    }

    pub async fn send_photo_path(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        caption: Option<&str>,
        parse_mode: Option<teloxide::types::ParseMode>,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        use teloxide::types::InputFile;
        let mut req = self.bot.send_photo(ChatId(chat_id), InputFile::file(path));
        if let Some(c) = caption {
            req = req.caption(c);
        }
        if let Some(pm) = parse_mode {
            req = req.parse_mode(pm);
        }
        let msg = req.await.map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: msg.id.0 as i64,
            date: msg.date.timestamp(),
        })
    }

    pub async fn send_document_path(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        caption: Option<&str>,
        filename: Option<&str>,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        use teloxide::types::InputFile;
        let mut file = InputFile::file(path);
        if let Some(name) = filename {
            file = file.file_name(name.to_string());
        }
        let mut req = self.bot.send_document(ChatId(chat_id), file);
        if let Some(c) = caption {
            req = req.caption(c);
        }
        let msg = req.await.map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: msg.id.0 as i64,
            date: msg.date.timestamp(),
        })
    }
```

- [ ] **Step 4: Run, confirm pass**

Run: `cargo test -p tg-client --test send`
Expected: all 6 pass (2 from Task 11 + 4 new).

- [ ] **Step 5: Commit**

```bash
git add crates/tg-client/
git commit -m "feat(tg-client): edit, delete, forward, chat_action, send_photo, send_document"
```

### Task 13: get_updates and get_me

**Files:**
- Modify: `crates/tg-client/src/client.rs`
- Create: `crates/tg-client/tests/updates.rs`

- [ ] **Step 1: Write failing test**

```rust
use tg_client::TgClient;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn get_updates_returns_serde_json_value() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": [
                {
                    "update_id": 100,
                    "message": {
                        "message_id": 1,
                        "date": 1_700_000_000,
                        "chat": { "id": 42, "type": "private", "first_name": "alice" },
                        "from": { "id": 42, "is_bot": false, "first_name": "alice" },
                        "text": "hello"
                    }
                }
            ]
        })))
        .mount(&server)
        .await;
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let updates = client.get_updates_raw(None, 0, None).await.unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["update_id"].as_i64(), Some(100));
}

#[tokio::test]
async fn get_me_returns_username() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "id": 555, "is_bot": true,
                "first_name": "MyBot", "username": "mybot"
            }
        })))
        .mount(&server)
        .await;
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let me = client.get_me().await.unwrap();
    assert_eq!(me.username.as_deref(), Some("mybot"));
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p tg-client --test updates`
Expected: methods not found.

- [ ] **Step 3: Extend `TgClient`**

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct BotIdentity {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: String,
}

impl TgClient {
    pub async fn get_me(&self) -> Result<BotIdentity, TgClientError> {
        let me = self.bot.get_me().await.map_err(map_teloxide_err)?;
        Ok(BotIdentity {
            id: me.id.0 as i64,
            username: me.username.clone(),
            first_name: me.first_name.clone(),
        })
    }

    /// Raw `getUpdates` returning the JSON array as-is. The updater normalizes
    /// these into `StoredMessage`s; keeping this raw means forward-compat with
    /// new update kinds without bumping `teloxide`.
    pub async fn get_updates_raw(
        &self,
        offset: Option<i64>,
        timeout_secs: u64,
        allowed_updates: Option<&[&str]>,
    ) -> Result<Vec<serde_json::Value>, TgClientError> {
        let url = self
            .api_base
            .join(&format!("bot{}/getUpdates", self.bot.token()))?;
        let mut body = serde_json::Map::new();
        if let Some(o) = offset {
            body.insert("offset".into(), serde_json::json!(o));
        }
        body.insert("timeout".into(), serde_json::json!(timeout_secs));
        if let Some(kinds) = allowed_updates {
            body.insert("allowed_updates".into(), serde_json::json!(kinds));
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs + 10))
            .build()?;
        let resp: serde_json::Value = client
            .post(url)
            .json(&serde_json::Value::Object(body))
            .send()
            .await?
            .json()
            .await?;
        if resp["ok"].as_bool() != Some(true) {
            let code = resp["error_code"].as_i64().unwrap_or(0) as i32;
            let desc = resp["description"].as_str().unwrap_or("").to_string();
            if code == 429 {
                let ra = resp["parameters"]["retry_after"].as_u64().unwrap_or(1) as u32;
                return Err(TgClientError::RateLimited { retry_after_secs: ra });
            }
            return Err(TgClientError::Api { code, description: desc });
        }
        let arr = resp["result"]
            .as_array()
            .cloned()
            .ok_or_else(|| TgClientError::Api {
                code: -1,
                description: "result is not an array".into(),
            })?;
        Ok(arr)
    }
}
```

Note: `teloxide::Bot::token()` returns the token string for URL construction. If your `teloxide` version hides it, store the token alongside `Bot` in `TgClient` and use that.

- [ ] **Step 4: Adjust `TgClient::new` to retain the token**

```rust
#[derive(Clone)]
pub struct TgClient {
    bot: teloxide::Bot,
    api_base: Url,
    token: String,
}

impl TgClient {
    pub fn new(token: String, api_base: Option<Url>) -> Result<Self, TgClientError> {
        let url = api_base.clone().unwrap_or_else(|| {
            "https://api.telegram.org".parse().expect("static URL parses")
        });
        let bot = teloxide::Bot::new(&token).set_api_url(url.clone());
        Ok(Self { bot, api_base: url, token })
    }
}
```

Then in `get_updates_raw`, use `self.token` instead of `self.bot.token()`. Update the `Debug` impl to keep the token field redacted (don't add it to the displayed fields).

- [ ] **Step 5: Run, confirm pass**

Run: `cargo test -p tg-client --test updates`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/tg-client/
git commit -m "feat(tg-client): get_me, get_updates_raw"
```

### Task 14: download_file by file_id

**Files:**
- Modify: `crates/tg-client/src/client.rs`
- Create: `crates/tg-client/tests/download.rs`

- [ ] **Step 1: Write failing test**

```rust
use tempfile::tempdir;
use tg_client::TgClient;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn download_writes_bytes_to_dest_path() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();

    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetFile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": { "file_id": "abc", "file_unique_id": "abc-u", "file_path": "documents/file_123.bin", "file_size": 5 }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/file/bot12345:fake/documents/file_123.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello"))
        .mount(&server)
        .await;

    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let dir = tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let bytes_written = client.download_file("abc", &dest).await.unwrap();
    assert_eq!(bytes_written, 5);
    assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p tg-client --test download`
Expected: `download_file` not found.

- [ ] **Step 3: Implement**

```rust
    pub async fn download_file(
        &self,
        file_id: &str,
        dest: &std::path::Path,
    ) -> Result<u64, TgClientError> {
        // 1) getFile to learn the path
        let get_file_url = self
            .api_base
            .join(&format!("bot{}/getFile", self.token))?;
        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(get_file_url)
            .json(&serde_json::json!({ "file_id": file_id }))
            .send()
            .await?
            .json()
            .await?;
        if resp["ok"].as_bool() != Some(true) {
            return Err(TgClientError::Api {
                code: resp["error_code"].as_i64().unwrap_or(0) as i32,
                description: resp["description"].as_str().unwrap_or("").into(),
            });
        }
        let file_path = resp["result"]["file_path"]
            .as_str()
            .ok_or_else(|| TgClientError::Download("missing file_path".into()))?;

        // 2) GET the file bytes
        let dl_url = self
            .api_base
            .join(&format!("file/bot{}/{}", self.token, file_path))?;
        let mut resp = client.get(dl_url).send().await?.error_for_status()?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TgClientError::Download(e.to_string()))?;
        }
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| TgClientError::Download(e.to_string()))?;
        let mut total = 0_u64;
        while let Some(chunk) = resp.chunk().await? {
            use tokio::io::AsyncWriteExt;
            file.write_all(&chunk).await.map_err(|e| TgClientError::Download(e.to_string()))?;
            total += chunk.len() as u64;
        }
        Ok(total)
    }
```

- [ ] **Step 4: Run, confirm pass**

Run: `cargo test -p tg-client --test download`
Expected: 1 passed.

- [ ] **Step 5: Lint**

Run: `cargo clippy -p tg-client --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/tg-client/
git commit -m "feat(tg-client): download_file by file_id"
```

---

## Milestone 5 — `tg-updater` crate

### Task 15: tg-updater skeleton with mapping function

**Files:**
- Create: `crates/tg-updater/Cargo.toml`
- Create: `crates/tg-updater/src/lib.rs`
- Create: `crates/tg-updater/src/error.rs`
- Create: `crates/tg-updater/src/mapping.rs`
- Create: `crates/tg-updater/tests/mapping.rs`

- [ ] **Step 1: Write `crates/tg-updater/Cargo.toml`**

```toml
[package]
name = "tg-updater"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
history = { workspace = true }
tg-client = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
wiremock = { workspace = true }
url = { workspace = true }
```

- [ ] **Step 2: Write `crates/tg-updater/src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UpdaterError {
    #[error("client error: {0}")]
    Client(#[from] tg_client::TgClientError),
    #[error("history error: {0}")]
    Store(#[from] history::HistoryError),
    #[error("update decode: {0}")]
    Decode(#[from] serde_json::Error),
}
```

- [ ] **Step 3: Write `crates/tg-updater/src/lib.rs`**

```rust
//! Telegram update poller: `getUpdates` → `history`.

pub mod error;
pub mod mapping;
mod r#loop;

pub use error::UpdaterError;
pub use r#loop::{Updater, UpdaterConfig};
pub use mapping::map_update;
```

- [ ] **Step 4: Write `crates/tg-updater/src/mapping.rs`**

```rust
use history::{ChatInfo, ChatKind, Direction, StoredMessage};
use serde_json::Value;

/// Convert a Telegram `Update` JSON into a (ChatInfo, StoredMessage) pair
/// representing the message we should persist. Returns `None` for updates
/// that don't carry a storable message (callback queries, etc.).
#[must_use]
pub fn map_update(update: &Value) -> Option<(ChatInfo, StoredMessage)> {
    let msg = update
        .get("message")
        .or_else(|| update.get("edited_message"))
        .or_else(|| update.get("channel_post"))
        .or_else(|| update.get("edited_channel_post"))?;
    let chat = msg.get("chat")?;

    let chat_id = chat.get("id")?.as_i64()?;
    let kind = match chat.get("type")?.as_str()? {
        "private" => ChatKind::Private,
        "group" => ChatKind::Group,
        "supergroup" => ChatKind::Supergroup,
        "channel" => ChatKind::Channel,
        _ => return None,
    };
    let title = chat.get("title").and_then(Value::as_str).map(str::to_string).or_else(|| {
        let first = chat.get("first_name").and_then(Value::as_str).unwrap_or("");
        let last = chat.get("last_name").and_then(Value::as_str).unwrap_or("");
        let combined = format!("{first} {last}").trim().to_string();
        (!combined.is_empty()).then_some(combined)
    });
    let username = chat.get("username").and_then(Value::as_str).map(str::to_string);

    let message_id = msg.get("message_id")?.as_i64()?;
    let date = msg.get("date")?.as_i64()?;
    let from = msg.get("from");
    let from_id = from.and_then(|f| f.get("id")).and_then(Value::as_i64);
    let from_name = from.and_then(|f| {
        let first = f.get("first_name").and_then(Value::as_str).unwrap_or("");
        let last = f.get("last_name").and_then(Value::as_str).unwrap_or("");
        let combined = format!("{first} {last}").trim().to_string();
        (!combined.is_empty()).then_some(combined)
    });
    let reply_to = msg.get("reply_to_message")
        .and_then(|r| r.get("message_id"))
        .and_then(Value::as_i64);
    let text = msg.get("text").or_else(|| msg.get("caption"))
        .and_then(Value::as_str).map(str::to_string);

    let (media_kind, media_file_id, media_meta) = extract_media(msg);

    let chat_info = ChatInfo {
        chat_id, kind, title, username,
        first_seen: date, last_seen: date,
    };
    let stored = StoredMessage {
        chat_id, message_id, date, from_id, from_name, reply_to, text,
        media_kind, media_file_id, media_meta,
        direction: Direction::In,
        raw: update.clone(),
    };
    Some((chat_info, stored))
}

fn extract_media(msg: &Value) -> (Option<String>, Option<String>, Option<Value>) {
    for (key, kind) in [
        ("photo", "photo"),
        ("document", "document"),
        ("voice", "voice"),
        ("video", "video"),
        ("animation", "animation"),
        ("audio", "audio"),
        ("sticker", "sticker"),
    ] {
        if let Some(v) = msg.get(key) {
            // photos come as arrays; pick the largest by file_size.
            let (file_id, meta) = if key == "photo" {
                if let Some(arr) = v.as_array() {
                    let largest = arr.iter().max_by_key(|p| {
                        p.get("file_size").and_then(Value::as_i64).unwrap_or(0)
                    });
                    match largest {
                        Some(p) => (
                            p.get("file_id").and_then(Value::as_str).map(str::to_string),
                            Some(p.clone()),
                        ),
                        None => (None, None),
                    }
                } else { (None, None) }
            } else {
                (
                    v.get("file_id").and_then(Value::as_str).map(str::to_string),
                    Some(v.clone()),
                )
            };
            return (Some(kind.into()), file_id, meta);
        }
    }
    (None, None, None)
}
```

- [ ] **Step 5: Stub `crates/tg-updater/src/loop.rs`**

```rust
use crate::UpdaterError;
use history::History;
use tg_client::TgClient;

#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    pub poll_timeout_secs: u64,
    pub allowed_update_kinds: Vec<String>,
    pub allowed_chats: Option<Vec<i64>>,
}

pub struct Updater {
    pub client: TgClient,
    pub store: History,
    pub config: UpdaterConfig,
}

impl Updater {
    pub async fn run(self) -> Result<std::convert::Infallible, UpdaterError> {
        unimplemented!("filled by Task 16")
    }
}
```

- [ ] **Step 6: Write the mapping test at `crates/tg-updater/tests/mapping.rs`**

```rust
use serde_json::json;
use tg_updater::map_update;

#[test]
fn maps_private_text_message() {
    let u = json!({
        "update_id": 1,
        "message": {
            "message_id": 7,
            "date": 1000,
            "chat": { "id": 42, "type": "private", "first_name": "alice" },
            "from": { "id": 42, "is_bot": false, "first_name": "alice" },
            "text": "hello"
        }
    });
    let (chat, msg) = map_update(&u).unwrap();
    assert_eq!(chat.chat_id, 42);
    assert_eq!(msg.text.as_deref(), Some("hello"));
    assert_eq!(msg.message_id, 7);
}

#[test]
fn maps_photo_uses_largest() {
    let u = json!({
        "update_id": 2,
        "message": {
            "message_id": 1, "date": 1,
            "chat": { "id": 1, "type": "private", "first_name": "x" },
            "photo": [
                { "file_id": "small", "file_size": 100 },
                { "file_id": "big", "file_size": 9999 }
            ]
        }
    });
    let (_, msg) = map_update(&u).unwrap();
    assert_eq!(msg.media_kind.as_deref(), Some("photo"));
    assert_eq!(msg.media_file_id.as_deref(), Some("big"));
}

#[test]
fn ignores_callback_query() {
    let u = json!({
        "update_id": 3,
        "callback_query": { "id": "cb", "from": { "id": 1, "is_bot": false, "first_name": "x" } }
    });
    assert!(map_update(&u).is_none());
}
```

- [ ] **Step 7: Run all 3, confirm pass**

Run: `cargo test -p tg-updater --test mapping`
Expected: 3 passed.

- [ ] **Step 8: Commit**

```bash
git add crates/tg-updater/
git commit -m "feat(tg-updater): crate skeleton, Update→StoredMessage mapping"
```

### Task 16: Updater loop with offset persistence, backoff, access filter

**Files:**
- Modify: `crates/tg-updater/src/loop.rs`
- Create: `crates/tg-updater/tests/loop.rs`

- [ ] **Step 1: Write the failing integration test**

```rust
use history::History;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tg_client::TgClient;
use tg_updater::{Updater, UpdaterConfig};
use url::Url;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

#[tokio::test]
async fn updater_consumes_batch_persists_offset_and_messages() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();

    // first call returns 1 update
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetUpdates"))
        .and(body_partial_json(json!({ "offset": 0 })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": [
                {
                    "update_id": 100,
                    "message": {
                        "message_id": 1, "date": 1_700_000_000,
                        "chat": { "id": 42, "type": "private", "first_name": "alice" },
                        "from": { "id": 42, "is_bot": false, "first_name": "alice" },
                        "text": "hello"
                    }
                }
            ]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // subsequent calls return empty
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "result": []
        })))
        .mount(&server)
        .await;

    let dir = tempdir().unwrap();
    let store = History::open(dir.path().join("h.db")).unwrap();
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let config = UpdaterConfig {
        poll_timeout_secs: 1,
        allowed_update_kinds: vec!["message".into()],
        allowed_chats: None,
    };
    let updater = Updater { client, store: store.clone(), config };

    let handle = tokio::spawn(updater.run());
    // give it time to process one batch
    tokio::time::sleep(Duration::from_millis(400)).await;
    handle.abort();

    // message landed
    let msg = store.get_message(42, 1).await.unwrap();
    assert_eq!(msg.text.as_deref(), Some("hello"));
    // offset persisted as 101 (last_update_id + 1)
    assert_eq!(store.kv_get("update_offset").await.unwrap().as_deref(), Some("101"));
}

#[tokio::test]
async fn updater_drops_updates_from_disallowed_chats() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": [
                {
                    "update_id": 1,
                    "message": {
                        "message_id": 1, "date": 1,
                        "chat": { "id": 999, "type": "private", "first_name": "stranger" },
                        "from": { "id": 999, "is_bot": false, "first_name": "stranger" },
                        "text": "spam"
                    }
                }
            ]
        })))
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let store = History::open(dir.path().join("h.db")).unwrap();
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let config = UpdaterConfig {
        poll_timeout_secs: 1,
        allowed_update_kinds: vec!["message".into()],
        allowed_chats: Some(vec![42]),
    };
    let updater = Updater { client, store: store.clone(), config };
    let handle = tokio::spawn(updater.run());
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.abort();

    // chat 999 was filtered → no chat row, no message row
    let chats = store.list_chats().await.unwrap();
    assert!(chats.is_empty());
}
```

- [ ] **Step 2: Confirm fail**

Run: `cargo test -p tg-updater --test r#loop` (note the raw identifier or rename `loop.rs` to `polling.rs` — see Step 3).

Actually: `loop` is a keyword. Rename `src/loop.rs` to `src/polling.rs` and adjust `lib.rs` accordingly. Update the test file name to `tests/polling.rs`.

Run: `cargo test -p tg-updater --test polling`
Expected: panic in `Updater::run` (`unimplemented!`).

- [ ] **Step 3: Implement `crates/tg-updater/src/polling.rs`**

Rename the file from `loop.rs` to `polling.rs`, update `lib.rs`:

```rust
pub mod error;
pub mod mapping;
mod polling;

pub use error::UpdaterError;
pub use mapping::map_update;
pub use polling::{Updater, UpdaterConfig};
```

Then in `polling.rs`:

```rust
use crate::{map_update, UpdaterError};
use history::History;
use std::collections::HashSet;
use std::time::Duration;
use tg_client::TgClient;

#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    pub poll_timeout_secs: u64,
    pub allowed_update_kinds: Vec<String>,
    pub allowed_chats: Option<Vec<i64>>,
}

pub struct Updater {
    pub client: TgClient,
    pub store: History,
    pub config: UpdaterConfig,
}

impl Updater {
    pub async fn run(self) -> Result<std::convert::Infallible, UpdaterError> {
        let allowed: Option<HashSet<i64>> = self
            .config
            .allowed_chats
            .as_ref()
            .map(|v| v.iter().copied().collect());
        let mut offset: Option<i64> = self
            .store
            .kv_get("update_offset")
            .await?
            .and_then(|s| s.parse().ok());
        let mut backoff = Duration::from_secs(1);
        let kinds_owned: Vec<String> = self.config.allowed_update_kinds.clone();

        loop {
            let kinds_refs: Vec<&str> = kinds_owned.iter().map(String::as_str).collect();
            let kinds_arg = if kinds_refs.is_empty() {
                None
            } else {
                Some(kinds_refs.as_slice())
            };

            let result = self
                .client
                .get_updates_raw(offset, self.config.poll_timeout_secs, kinds_arg)
                .await;

            match result {
                Ok(updates) => {
                    backoff = Duration::from_secs(1);
                    let mut max_update_id: Option<i64> = None;
                    for u in &updates {
                        let id = u.get("update_id").and_then(|v| v.as_i64());
                        if let Some(i) = id {
                            max_update_id = Some(max_update_id.map_or(i, |m| m.max(i)));
                        }
                        let Some((chat, msg)) = map_update(u) else { continue };
                        if let Some(a) = &allowed {
                            if !a.contains(&chat.chat_id) {
                                tracing::debug!(chat_id = chat.chat_id, "dropped: disallowed chat");
                                continue;
                            }
                        }
                        if let Err(e) = self.store.upsert_chat(&chat).await {
                            tracing::error!(error = %e, "upsert_chat failed");
                            continue;
                        }
                        if let Err(e) = self.store.insert_message(&msg).await {
                            tracing::error!(error = %e, "insert_message failed");
                            continue;
                        }
                    }
                    if let Some(m) = max_update_id {
                        let next = m + 1;
                        if let Err(e) = self
                            .store
                            .kv_put("update_offset", &next.to_string())
                            .await
                        {
                            tracing::error!(error = %e, "persist offset failed");
                        } else {
                            offset = Some(next);
                        }
                    }
                }
                Err(tg_client::TgClientError::RateLimited { retry_after_secs }) => {
                    tracing::warn!(retry_after_secs, "rate limited by Telegram");
                    tokio::time::sleep(Duration::from_secs(u64::from(retry_after_secs))).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, ?backoff, "getUpdates failed; backing off");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run tests, confirm pass**

Run: `cargo test -p tg-updater --test polling`
Expected: 2 passed.

- [ ] **Step 5: Lint**

Run: `cargo clippy -p tg-updater --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/tg-updater/
git commit -m "feat(tg-updater): polling loop with offset, backoff, access filter"
```

---

## Milestone 6 — `mcp-server` binary

### Task 17: mcp-server crate skeleton and config parsing

**Files:**
- Create: `crates/mcp-server/Cargo.toml`
- Create: `crates/mcp-server/src/main.rs`
- Create: `crates/mcp-server/src/config.rs`
- Create: `crates/mcp-server/src/tools_io.rs`
- Create: `crates/mcp-server/src/error.rs`

- [ ] **Step 1: Write `crates/mcp-server/Cargo.toml`**

```toml
[package]
name = "mcp-server"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[[bin]]
name = "TelegramMCP"
path = "src/main.rs"

[dependencies]
aliases = { workspace = true }
history = { workspace = true }
tg-client = { workspace = true }
tg-updater = { workspace = true }

rmcp = { workspace = true }
schemars = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
url = { workspace = true }
thiserror = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
wiremock = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 2: Write `crates/mcp-server/src/config.rs`**

```rust
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub bot: BotConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub updater: UpdaterConfig,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub aliases: BTreeMap<String, i64>,
    #[serde(default)]
    pub access: AccessConfig,
}

#[derive(Debug, Deserialize)]
pub struct BotConfig {
    pub token: Option<String>,
    pub token_env: Option<String>,
    pub api_base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StorageConfig {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct UpdaterConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_poll_timeout")]
    pub poll_timeout_secs: u64,
    #[serde(default = "default_allowed_kinds")]
    pub allowed_update_kinds: Vec<String>,
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_timeout_secs: default_poll_timeout(),
            allowed_update_kinds: default_allowed_kinds(),
        }
    }
}

fn default_true() -> bool { true }
fn default_poll_timeout() -> u64 { 30 }
fn default_allowed_kinds() -> Vec<String> {
    vec!["message".into(), "edited_message".into(),
         "channel_post".into(), "edited_channel_post".into()]
}

#[derive(Debug, Default, Deserialize)]
pub struct RetentionConfig {
    pub max_age_days: Option<u64>,
    pub max_messages_total: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AccessConfig {
    #[serde(default)]
    pub allowed_chats: Vec<AliasOrId>,
    #[serde(default)]
    pub allowed_send_targets: Vec<AliasOrId>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AliasOrId {
    Id(i64),
    Name(String),
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config: {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw).context("parsing config TOML")?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn resolved_token(&self) -> Result<String> {
        if let Some(env_key) = &self.bot.token_env {
            return std::env::var(env_key)
                .with_context(|| format!("env var {env_key} not set"));
        }
        if let Some(t) = &self.bot.token {
            tracing::warn!("bot.token set inline in config; prefer bot.token_env");
            return Ok(t.clone());
        }
        bail!("must set either [bot] token_env or [bot] token");
    }

    fn validate(&self) -> Result<()> {
        if self.updater.poll_timeout_secs < 1 || self.updater.poll_timeout_secs > 50 {
            bail!("[updater] poll_timeout_secs must be in [1, 50]");
        }
        if let Some(parent) = self.storage.path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                bail!("storage.path parent does not exist: {}", parent.display());
            }
        }
        for entry in self.access.allowed_chats.iter().chain(self.access.allowed_send_targets.iter()) {
            if let AliasOrId::Name(n) = entry {
                if !self.aliases.contains_key(n) {
                    bail!("access list references unknown alias: {n}");
                }
            }
        }
        if self.bot.token.is_none() && self.bot.token_env.is_none() {
            bail!("[bot] requires either token or token_env");
        }
        Ok(())
    }

    pub fn resolve_id_list(&self, list: &[AliasOrId]) -> Result<Vec<i64>> {
        list.iter()
            .map(|e| match e {
                AliasOrId::Id(id) => Ok(*id),
                AliasOrId::Name(n) => self
                    .aliases
                    .get(n)
                    .copied()
                    .with_context(|| format!("unknown alias in access list: {n}")),
            })
            .collect()
    }
}
```

- [ ] **Step 3: Write `crates/mcp-server/src/error.rs`**

```rust
use rmcp::Error as McpError;

pub fn client_err_to_mcp(e: &tg_client::TgClientError) -> McpError {
    use tg_client::TgClientError as E;
    let msg = e.to_string();
    match e {
        E::Http(_) | E::Teloxide(_) | E::Download(_) => McpError::internal_error(msg, None),
        E::Api { .. } | E::RateLimited { .. } | E::UnknownAlias(_)
        | E::InvalidChat(_) | E::InvalidUrl(_) => McpError::invalid_params(msg, None),
    }
}

pub fn history_err_to_mcp(e: &history::HistoryError) -> McpError {
    use history::HistoryError as E;
    let msg = e.to_string();
    match e {
        E::NotFound { .. } | E::Corruption(_) => McpError::invalid_params(msg, None),
        _ => McpError::internal_error(msg, None),
    }
}

pub fn alias_err_to_mcp(e: &aliases::UnknownAlias) -> McpError {
    McpError::invalid_params(e.to_string(), None)
}
```

- [ ] **Step 4: Write `crates/mcp-server/src/tools_io.rs`**

```rust
//! Tool input/output types. `serde::Deserialize` + `schemars::JsonSchema`
//! so the input schema is generated automatically by `schema_obj`.

use aliases::ChatRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    pub chat: ChatRef,
    pub text: String,
    #[serde(default)] pub parse_mode: Option<String>,
    #[serde(default)] pub reply_to: Option<i64>,
    #[serde(default)] pub silent: Option<bool>,
    #[serde(default)] pub link_preview: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendPhotoInput {
    pub chat: ChatRef,
    pub path: PathBuf,
    #[serde(default)] pub caption: Option<String>,
    #[serde(default)] pub parse_mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendDocumentInput {
    pub chat: ChatRef,
    pub path: PathBuf,
    #[serde(default)] pub caption: Option<String>,
    #[serde(default)] pub filename: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditMessageInput {
    pub chat: ChatRef,
    pub message_id: i64,
    pub text: String,
    #[serde(default)] pub parse_mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteMessageInput {
    pub chat: ChatRef,
    pub message_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForwardMessageInput {
    pub from_chat: ChatRef,
    pub message_id: i64,
    pub to_chat: ChatRef,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChatActionInput {
    pub chat: ChatRef,
    pub action: String, // typing, upload_photo, ...
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SendMessageOutput {
    pub chat_id: i64,
    pub message_id: i64,
    pub date: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListChatsInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetChatInput { pub chat: ChatRef }

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistoryMessagesInput {
    pub chat: ChatRef,
    #[serde(default)] pub before_message_id: Option<i64>,
    #[serde(default)] pub after_message_id: Option<i64>,
    #[serde(default = "default_limit")] pub limit: i64,
}
fn default_limit() -> i64 { 50 }

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistorySearchInput {
    pub query: String,
    #[serde(default)] pub chat: Option<ChatRef>,
    #[serde(default)] pub since: Option<i64>,
    #[serde(default)] pub until: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMessageInput {
    pub chat: ChatRef,
    pub message_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkReadInput {
    pub chat: ChatRef,
    pub up_to_message_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DownloadInput {
    pub chat: ChatRef,
    pub message_id: i64,
    pub dest_path: PathBuf,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BotWhoamiInput {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAliasesInput {}
```

- [ ] **Step 5: Stub `crates/mcp-server/src/main.rs`**

```rust
//! TelegramMCP — MCP server binary, stdio transport.
mod config;
mod error;
mod tools_io;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("TelegramMCP starting (skeleton — Task 18 wires rmcp)");
    Ok(())
}
```

- [ ] **Step 6: Verify build**

Run: `cargo build -p mcp-server`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/mcp-server/
git commit -m "feat(mcp-server): crate skeleton, config schema, tool I/O types"
```

### Task 18: Wire rmcp dispatch + bot identity tools

**Files:**
- Modify: `crates/mcp-server/src/main.rs`

- [ ] **Step 1: Replace `crates/mcp-server/src/main.rs` body**

```rust
//! TelegramMCP — MCP server binary, stdio transport.

mod config;
mod error;
mod tools_io;

use anyhow::{Context, Result};
use rmcp::{
    Error as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParam, CallToolResult, Content, Implementation, ListToolsResult,
        PaginatedRequestParam, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    transport::stdio,
};
use schemars::JsonSchema;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use crate::config::{AliasOrId, Config};
use crate::error::{alias_err_to_mcp, client_err_to_mcp, history_err_to_mcp};
use crate::tools_io::*;

use aliases::{Aliases, ChatRef};
use history::History;
use tg_client::TgClient;

#[derive(Clone)]
struct State {
    bot: TgClient,
    store: History,
    aliases: Aliases,
    allowed_send_targets: Option<Vec<i64>>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("bot", &self.bot)
            .field("allowed_send_targets", &self.allowed_send_targets)
            .finish()
    }
}

#[derive(Debug, Clone)]
struct Server(Arc<State>);

fn schema_obj<T: JsonSchema>() -> Arc<Map<String, Value>> {
    let mut generator = schemars::r#gen::SchemaGenerator::default();
    let schema = T::json_schema(&mut generator);
    let value = serde_json::to_value(&schema).expect("schema serializes");
    let mut obj = match value {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    obj.remove("$schema");
    obj.remove("title");
    Arc::new(obj)
}

fn tool(name: &'static str, desc: &'static str, schema: Arc<Map<String, Value>>) -> Tool {
    Tool { name: name.into(), description: Some(desc.into()), input_schema: schema, annotations: None }
}

fn parse_args<T: serde::de::DeserializeOwned>(args: Option<&Map<String, Value>>) -> Result<T, McpError> {
    let value = args.map_or(Value::Object(Map::new()), |m| Value::Object(m.clone()));
    serde_json::from_value(value)
        .map_err(|e| McpError::invalid_params(format!("invalid arguments: {e}"), None))
}

fn ok_json<T: serde::Serialize>(v: &T) -> Result<CallToolResult, McpError> {
    let payload = serde_json::to_string_pretty(v)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(payload)]))
}

fn resolve_chat(aliases: &Aliases, r: &ChatRef) -> Result<i64, McpError> {
    aliases.resolve(r).map_err(|e| alias_err_to_mcp(&e))
}

fn check_send_allowed(state: &State, chat_id: i64) -> Result<(), McpError> {
    if let Some(list) = &state.allowed_send_targets {
        if !list.contains(&chat_id) {
            return Err(McpError::invalid_params(
                format!("chat {chat_id} is not in allowed_send_targets"),
                None,
            ));
        }
    }
    Ok(())
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "TelegramMCP".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some("Telegram Bot API + local history. Send messages, read incoming, search history.".into()),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: vec![
                tool("tg_bot_whoami", "Return bot id, username, and display name.",
                     schema_obj::<BotWhoamiInput>()),
                tool("tg_bot_list_aliases", "Return configured chat-name → chat_id map.",
                     schema_obj::<ListAliasesInput>()),
                // additional tools wired in later tasks
            ],
            next_cursor: None,
        })
    }

    #[allow(clippy::too_many_lines)] // matches sibling project's dispatch style
    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        match request.name.as_ref() {
            "tg_bot_whoami" => {
                let _: BotWhoamiInput = parse_args(request.arguments.as_ref())?;
                let me = self.0.bot.get_me().await.map_err(|e| client_err_to_mcp(&e))?;
                ok_json(&me)
            }
            "tg_bot_list_aliases" => {
                let _: ListAliasesInput = parse_args(request.arguments.as_ref())?;
                ok_json(self.0.aliases.as_map())
            }
            other => Err(McpError::method_not_found::<rmcp::model::CallToolRequestMethod>(
                format!("unknown tool: {other}"),
            )),
        }
    }
}

fn parse_cli() -> Result<Option<PathBuf>> {
    let mut args = std::env::args().skip(1);
    let mut cfg = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => {
                let v = args.next().context("--config requires a path argument")?;
                cfg = Some(PathBuf::from(v));
            }
            "--help" | "-h" => {
                eprintln!(
                    "TelegramMCP v{} — MCP server for the Telegram Bot API.\n\
                     \n\
                     USAGE:\n  TelegramMCP --config <path>\n\
                     \n\
                     ENV:\n  TELEGRAM_MCP_LOG    tracing-subscriber filter (default: info).",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(cfg)
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_env("TELEGRAM_MCP_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let config_path = parse_cli()?.context("--config <path> is required")?;
    let cfg = Config::load(&config_path)?;

    let token = cfg.resolved_token()?;
    let api_base = cfg
        .bot
        .api_base_url
        .as_deref()
        .map(url::Url::parse)
        .transpose()
        .context("invalid bot.api_base_url")?;
    let client = TgClient::new(token, api_base.clone())
        .context("constructing TgClient")?;

    let store = History::open(&cfg.storage.path)
        .with_context(|| format!("opening history at {}", cfg.storage.path.display()))?;

    let aliases = Aliases::new(cfg.aliases.clone());

    let allowed_send_targets = if cfg.access.allowed_send_targets.is_empty() {
        None
    } else {
        Some(cfg.resolve_id_list(&cfg.access.allowed_send_targets)?)
    };

    let state = Arc::new(State {
        bot: client.clone(),
        store: store.clone(),
        aliases,
        allowed_send_targets,
    });

    // Spawn the updater task if enabled.
    if cfg.updater.enabled {
        let allowed_chats = if cfg.access.allowed_chats.is_empty() {
            None
        } else {
            Some(cfg.resolve_id_list(&cfg.access.allowed_chats)?)
        };
        let updater_cfg = tg_updater::UpdaterConfig {
            poll_timeout_secs: cfg.updater.poll_timeout_secs,
            allowed_update_kinds: cfg.updater.allowed_update_kinds.clone(),
            allowed_chats,
        };
        let updater = tg_updater::Updater {
            client,
            store,
            config: updater_cfg,
        };
        tokio::spawn(async move {
            match updater.run().await {
                Ok(never) => match never {},
                Err(e) => tracing::error!(error = %e, "updater loop terminated"),
            }
        });
    }

    let server = Server(state);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build -p mcp-server`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/mcp-server/
git commit -m "feat(mcp-server): rmcp dispatch wired, tg_bot_whoami and tg_bot_list_aliases tools"
```

### Task 19: Outbound tools — tg_send_message

**Files:**
- Modify: `crates/mcp-server/src/main.rs`

- [ ] **Step 1: Add tool to `list_tools` registration**

In the `tools: vec![...]` array, add:

```rust
                tool(
                    "tg_send_message",
                    "Send a text message to a chat. `chat` accepts a numeric chat_id or a \
                     configured alias. Returns the sent message id + date. The message is also \
                     written to local history with direction='out'.",
                    schema_obj::<SendMessageInput>(),
                ),
```

- [ ] **Step 2: Add match arm to `call_tool`**

```rust
            "tg_send_message" => {
                let input: SendMessageInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let parse_mode = input.parse_mode.as_deref().and_then(parse_parse_mode);
                let sent = self.0.bot
                    .send_message(
                        chat_id,
                        &input.text,
                        parse_mode,
                        input.reply_to,
                        input.silent.unwrap_or(false),
                        input.link_preview.unwrap_or(true),
                    )
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                // Mirror into history as direction='out'.
                let chat_info = history::ChatInfo {
                    chat_id: sent.chat_id,
                    kind: history::ChatKind::Private, // overwritten on next inbound update
                    title: None, username: None,
                    first_seen: sent.date, last_seen: sent.date,
                };
                self.0.store.upsert_chat(&chat_info).await.map_err(|e| history_err_to_mcp(&e))?;
                self.0.store.insert_message(&history::StoredMessage {
                    chat_id: sent.chat_id,
                    message_id: sent.message_id,
                    date: sent.date,
                    from_id: None, from_name: None, reply_to: input.reply_to,
                    text: Some(input.text.clone()),
                    media_kind: None, media_file_id: None, media_meta: None,
                    direction: history::Direction::Out,
                    raw: serde_json::json!({ "outbound": true }),
                }).await.map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&SendMessageOutput {
                    chat_id: sent.chat_id,
                    message_id: sent.message_id,
                    date: sent.date,
                })
            }
```

- [ ] **Step 3: Add helper to map `parse_mode` strings**

Append near the other helpers in `main.rs`:

```rust
fn parse_parse_mode(s: &str) -> Option<teloxide::types::ParseMode> {
    use teloxide::types::ParseMode;
    match s.to_ascii_lowercase().as_str() {
        "markdown" => Some(ParseMode::MarkdownV2),
        "markdownv2" => Some(ParseMode::MarkdownV2),
        "html" => Some(ParseMode::Html),
        _ => None,
    }
}
```

- [ ] **Step 4: Verify build**

Run: `cargo build -p mcp-server`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mcp-server/
git commit -m "feat(mcp-server): tg_send_message tool with outbound mirror to history"
```

### Task 20: Outbound tools — send_photo, send_document, edit, delete, forward, chat_action

**Files:**
- Modify: `crates/mcp-server/src/main.rs`

- [ ] **Step 1: Register tools in `list_tools`**

Append to the `tools:` vec:

```rust
                tool("tg_send_photo",
                     "Send a photo from a local path. Caption optional. Mirrors to history.",
                     schema_obj::<SendPhotoInput>()),
                tool("tg_send_document",
                     "Send a document (any file) from a local path. Caption + custom filename optional.",
                     schema_obj::<SendDocumentInput>()),
                tool("tg_edit_message",
                     "Edit the text of a previously-sent message. Returns the updated message stamp.",
                     schema_obj::<EditMessageInput>()),
                tool("tg_delete_message",
                     "Delete a message by chat + message_id. Bot can only delete its own messages \
                      (or other users' messages if it has admin rights).",
                     schema_obj::<DeleteMessageInput>()),
                tool("tg_forward_message",
                     "Forward a message from one chat to another. Returns the forwarded message id.",
                     schema_obj::<ForwardMessageInput>()),
                tool("tg_send_chat_action",
                     "Show a 'typing'/'uploading'/etc. indicator in the chat for ~5s. \
                      action: typing | upload_photo | record_voice | upload_voice | upload_document \
                      | choose_sticker | find_location | record_video_note | upload_video_note",
                     schema_obj::<ChatActionInput>()),
```

- [ ] **Step 2: Add helper `chat_action_from_str` near `parse_parse_mode`**

```rust
fn chat_action_from_str(s: &str) -> Option<teloxide::types::ChatAction> {
    use teloxide::types::ChatAction as A;
    Some(match s {
        "typing" => A::Typing,
        "upload_photo" => A::UploadPhoto,
        "record_video" => A::RecordVideo,
        "upload_video" => A::UploadVideo,
        "record_voice" => A::RecordVoice,
        "upload_voice" => A::UploadVoice,
        "upload_document" => A::UploadDocument,
        "find_location" => A::FindLocation,
        "record_video_note" => A::RecordVideoNote,
        "upload_video_note" => A::UploadVideoNote,
        "choose_sticker" => A::ChooseSticker,
        _ => return None,
    })
}
```

- [ ] **Step 3: Add match arms in `call_tool`**

```rust
            "tg_send_photo" => {
                let input: SendPhotoInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let pm = input.parse_mode.as_deref().and_then(parse_parse_mode);
                let sent = self.0.bot.send_photo_path(chat_id, &input.path, input.caption.as_deref(), pm)
                    .await.map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(&self.0.store, &sent, input.caption.as_deref(), Some("photo")).await?;
                ok_json(&SendMessageOutput { chat_id: sent.chat_id, message_id: sent.message_id, date: sent.date })
            }
            "tg_send_document" => {
                let input: SendDocumentInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let sent = self.0.bot.send_document_path(
                    chat_id, &input.path, input.caption.as_deref(), input.filename.as_deref()
                ).await.map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(&self.0.store, &sent, input.caption.as_deref(), Some("document")).await?;
                ok_json(&SendMessageOutput { chat_id: sent.chat_id, message_id: sent.message_id, date: sent.date })
            }
            "tg_edit_message" => {
                let input: EditMessageInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let pm = input.parse_mode.as_deref().and_then(parse_parse_mode);
                let sent = self.0.bot.edit_message_text(chat_id, input.message_id, &input.text, pm)
                    .await.map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(&self.0.store, &sent, Some(&input.text), None).await?;
                ok_json(&SendMessageOutput { chat_id: sent.chat_id, message_id: sent.message_id, date: sent.date })
            }
            "tg_delete_message" => {
                let input: DeleteMessageInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                self.0.bot.delete_message(chat_id, input.message_id).await.map_err(|e| client_err_to_mcp(&e))?;
                ok_json(&serde_json::json!({ "deleted": true, "chat_id": chat_id, "message_id": input.message_id }))
            }
            "tg_forward_message" => {
                let input: ForwardMessageInput = parse_args(request.arguments.as_ref())?;
                let from_chat = resolve_chat(&self.0.aliases, &input.from_chat)?;
                let to_chat = resolve_chat(&self.0.aliases, &input.to_chat)?;
                check_send_allowed(&self.0, to_chat)?;
                let sent = self.0.bot.forward_message(from_chat, input.message_id, to_chat)
                    .await.map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(&self.0.store, &sent, None, None).await?;
                ok_json(&SendMessageOutput { chat_id: sent.chat_id, message_id: sent.message_id, date: sent.date })
            }
            "tg_send_chat_action" => {
                let input: ChatActionInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let action = chat_action_from_str(&input.action)
                    .ok_or_else(|| McpError::invalid_params(format!("unknown action: {}", input.action), None))?;
                self.0.bot.send_chat_action(chat_id, action).await.map_err(|e| client_err_to_mcp(&e))?;
                ok_json(&serde_json::json!({ "ok": true }))
            }
```

- [ ] **Step 4: Add `mirror_outbound` helper near the parsing helpers**

```rust
async fn mirror_outbound(
    store: &History,
    sent: &tg_client::SentMessage,
    text: Option<&str>,
    media_kind: Option<&str>,
) -> Result<(), McpError> {
    let chat_info = history::ChatInfo {
        chat_id: sent.chat_id,
        kind: history::ChatKind::Private,
        title: None, username: None,
        first_seen: sent.date, last_seen: sent.date,
    };
    store.upsert_chat(&chat_info).await.map_err(|e| history_err_to_mcp(&e))?;
    store.insert_message(&history::StoredMessage {
        chat_id: sent.chat_id,
        message_id: sent.message_id,
        date: sent.date,
        from_id: None, from_name: None, reply_to: None,
        text: text.map(str::to_string),
        media_kind: media_kind.map(str::to_string),
        media_file_id: None, media_meta: None,
        direction: history::Direction::Out,
        raw: serde_json::json!({ "outbound": true }),
    }).await.map_err(|e| history_err_to_mcp(&e))?;
    Ok(())
}
```

(Refactor `tg_send_message` from Task 19 to call `mirror_outbound` for DRY.)

- [ ] **Step 5: Build and lint**

```
cargo build -p mcp-server
cargo clippy -p mcp-server --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/mcp-server/
git commit -m "feat(mcp-server): send_photo, send_document, edit, delete, forward, chat_action"
```

### Task 21: History tools — list_chats, get_chat, messages, search, get_message, mark_read, download

**Files:**
- Modify: `crates/mcp-server/src/main.rs`

- [ ] **Step 1: Register in `list_tools`**

```rust
                tool("tg_history_list_chats",
                     "List chats the bot has seen, with last-message timestamp + unread count.",
                     schema_obj::<ListChatsInput>()),
                tool("tg_history_get_chat",
                     "Return metadata for one chat: kind, title, username, first/last seen.",
                     schema_obj::<GetChatInput>()),
                tool("tg_history_messages",
                     "Paginated messages from a chat, newest-first. before_message_id and \
                      after_message_id are message-id cursors, exclusive. limit defaults to 50.",
                     schema_obj::<HistoryMessagesInput>()),
                tool("tg_history_search",
                     "Full-text search across stored messages (FTS5). Optionally scope to a chat \
                      or time window (unix seconds).",
                     schema_obj::<HistorySearchInput>()),
                tool("tg_history_get_message",
                     "Fetch a single stored message by (chat, message_id).",
                     schema_obj::<GetMessageInput>()),
                tool("tg_history_mark_read",
                     "Move the local unread baseline to this message_id, so subsequent \
                      list_chats reports unread_count from there forward.",
                     schema_obj::<MarkReadInput>()),
                tool("tg_history_download",
                     "Download the media attached to a stored message to a local path. Uses the \
                      stored Telegram file_id; fetches bytes from the Bot API on demand.",
                     schema_obj::<DownloadInput>()),
```

- [ ] **Step 2: Add match arms**

```rust
            "tg_history_list_chats" => {
                let _: ListChatsInput = parse_args(request.arguments.as_ref())?;
                let chats = self.0.store.list_chats().await.map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&chats)
            }
            "tg_history_get_chat" => {
                let input: GetChatInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                let c = self.0.store.get_chat(chat_id).await.map_err(|e| history_err_to_mcp(&e))?;
                match c {
                    Some(info) => ok_json(&info),
                    None => Err(McpError::invalid_params(format!("chat {chat_id} not in history"), None)),
                }
            }
            "tg_history_messages" => {
                let input: HistoryMessagesInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                let msgs = self.0.store
                    .messages(chat_id, input.before_message_id, input.after_message_id, input.limit)
                    .await.map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&msgs)
            }
            "tg_history_search" => {
                let input: HistorySearchInput = parse_args(request.arguments.as_ref())?;
                let chat_id = match &input.chat {
                    Some(r) => Some(resolve_chat(&self.0.aliases, r)?),
                    None => None,
                };
                let hits = self.0.store.search(&input.query, chat_id, input.since, input.until)
                    .await.map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&hits)
            }
            "tg_history_get_message" => {
                let input: GetMessageInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                let m = self.0.store.get_message(chat_id, input.message_id)
                    .await.map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&m)
            }
            "tg_history_mark_read" => {
                let input: MarkReadInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                self.0.store.mark_read(chat_id, input.up_to_message_id)
                    .await.map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&serde_json::json!({ "chat_id": chat_id, "baseline": input.up_to_message_id }))
            }
            "tg_history_download" => {
                let input: DownloadInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                let m = self.0.store.get_message(chat_id, input.message_id)
                    .await.map_err(|e| history_err_to_mcp(&e))?;
                let file_id = m.media_file_id.as_deref().ok_or_else(|| {
                    McpError::invalid_params(
                        format!("message {chat_id}/{} has no media", input.message_id), None)
                })?;
                let bytes = self.0.bot.download_file(file_id, &input.dest_path)
                    .await.map_err(|e| client_err_to_mcp(&e))?;
                ok_json(&serde_json::json!({
                    "dest_path": input.dest_path,
                    "bytes": bytes,
                }))
            }
```

- [ ] **Step 3: Build and lint**

```
cargo build -p mcp-server
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/mcp-server/
git commit -m "feat(mcp-server): history tools (list_chats, get_chat, messages, search, get_message, mark_read, download)"
```

---

## Milestone 7 — End-to-end smoke test

### Task 22: e2e smoke test scaffolding

**Files:**
- Create: `crates/mcp-server/tests/smoke.rs`
- Create: `crates/mcp-server/tests/common/mod.rs`

- [ ] **Step 1: Reference the sibling smoke harness as the canonical pattern**

Pattern to mirror (from `d:\Work\Programming\MCP\FileSystem\crates\mcp-server\tests\smoke.rs`):
- Spawn the binary as a child process with stdio piped.
- Write JSON-RPC `initialize` request to stdin; read response from stdout.
- Send `tools/call` requests, parse responses.

- [ ] **Step 2: Write `crates/mcp-server/tests/common/mod.rs`**

```rust
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::path::PathBuf;

pub struct McpClient {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: BufReader<ChildStdout>,
    pub next_id: u64,
}

impl McpClient {
    pub fn spawn(bin: &PathBuf, config: &PathBuf) -> Self {
        let mut child = Command::new(bin)
            .args(["--config", config.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn binary");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self { child, stdin, stdout, next_id: 1 }
    }

    pub fn send(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        });
        writeln!(self.stdin, "{}", req).unwrap();
        self.stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            if line.trim().is_empty() { continue; }
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                return v;
            }
            // notifications (no id) are ignored
        }
    }

    pub fn initialize(&mut self) {
        let resp = self.send("initialize", serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "smoke", "version": "0" }
        }));
        assert!(resp["result"].is_object(), "initialize response: {resp}");
        // Required by MCP: notify the server that initialization is done.
        let notify = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        });
        writeln!(self.stdin, "{}", notify).unwrap();
        self.stdin.flush().unwrap();
    }

    pub fn call_tool(&mut self, name: &str, args: serde_json::Value) -> serde_json::Value {
        self.send("tools/call", serde_json::json!({
            "name": name, "arguments": args
        }))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn binary_path() -> PathBuf {
    // cargo sets this for the bin target the test depends on
    PathBuf::from(env!("CARGO_BIN_EXE_TelegramMCP"))
}
```

- [ ] **Step 3: Write `crates/mcp-server/tests/smoke.rs`**

```rust
mod common;

use common::{binary_path, McpClient};
use std::io::Write;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_config(api_base: &str, db: &std::path::Path, alias_id: i64) -> String {
    format!(r#"
[bot]
token = "12345:fake"
api_base_url = "{api_base}"

[storage]
path = "{db}"

[updater]
enabled = false

[aliases]
test = {alias_id}
"#, db = db.display().to_string().replace('\\', "/"))
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": { "id": 555, "is_bot": true, "first_name": "TestBot", "username": "testbot" }
        })))
        .mount(&server)
        .await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    std::fs::write(&cfg_path, make_config(&server.uri(), &db_path, 42)).unwrap();

    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    let resp = client.call_tool("tg_bot_whoami", serde_json::json!({}));
    let content = &resp["result"]["content"][0]["text"];
    let parsed: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(parsed["username"], "testbot");
}
```

- [ ] **Step 4: Run, confirm pass**

Run: `cargo test -p mcp-server --test smoke whoami_round_trip`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/mcp-server/tests/
git commit -m "test(mcp-server): smoke harness + whoami round trip"
```

### Task 23: e2e — send_message via fake API, history readback

**Files:**
- Modify: `crates/mcp-server/tests/smoke.rs`

- [ ] **Step 1: Append two more tests**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn send_message_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 7, "date": 1_700_000_000,
                "chat": { "id": 42, "type": "private" }
            }
        })))
        .mount(&server).await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    std::fs::write(&cfg_path, make_config(&server.uri(), &db_path, 42)).unwrap();

    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    let resp = client.call_tool("tg_send_message", serde_json::json!({
        "chat": "test", "text": "hello"
    }));
    let content = &resp["result"]["content"][0]["text"];
    let parsed: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(parsed["message_id"], 7);
    assert_eq!(parsed["chat_id"], 42);
}

#[tokio::test(flavor = "multi_thread")]
async fn send_then_history_messages_readback() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 1, "date": 1_700_000_000,
                "chat": { "id": 42, "type": "private" }
            }
        })))
        .mount(&server).await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    std::fs::write(&cfg_path, make_config(&server.uri(), &db_path, 42)).unwrap();
    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    let _ = client.call_tool("tg_send_message", serde_json::json!({
        "chat": "test", "text": "stored"
    }));
    let resp = client.call_tool("tg_history_messages", serde_json::json!({
        "chat": "test", "limit": 10
    }));
    let content = &resp["result"]["content"][0]["text"];
    let parsed: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(parsed[0]["text"], "stored");
    assert_eq!(parsed[0]["direction"], "out");
}

#[tokio::test(flavor = "multi_thread")]
async fn send_to_disallowed_chat_returns_error() {
    let server = MockServer::start().await;
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    let cfg = format!(r#"
[bot]
token = "12345:fake"
api_base_url = "{}"

[storage]
path = "{}"

[updater]
enabled = false

[aliases]
allowed = 100
stranger = 200

[access]
allowed_send_targets = ["allowed"]
"#, server.uri(), db_path.display().to_string().replace('\\', "/"));
    std::fs::write(&cfg_path, cfg).unwrap();

    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    let resp = client.call_tool("tg_send_message", serde_json::json!({
        "chat": "stranger", "text": "blocked"
    }));
    assert!(resp.get("error").is_some(), "expected an error response, got {resp}");
}
```

- [ ] **Step 2: Run, confirm pass**

Run: `cargo test -p mcp-server --test smoke`
Expected: 4 passed (whoami + 3 new).

- [ ] **Step 3: Commit**

```bash
git add crates/mcp-server/tests/
git commit -m "test(mcp-server): e2e send_message, history readback, send-target allowlist"
```

### Task 24: e2e — updater enabled path, search returns hit

**Files:**
- Modify: `crates/mcp-server/tests/smoke.rs`

- [ ] **Step 1: Append**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn updater_enabled_flows_inbound_to_history_search() {
    let server = MockServer::start().await;

    // updater hits getUpdates — return one batch then empties.
    Mock::given(method("POST")).and(path("/bot12345:fake/GetUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": [{
                "update_id": 1,
                "message": {
                    "message_id": 5, "date": 1_700_000_000,
                    "chat": { "id": 42, "type": "private", "first_name": "alice" },
                    "from": { "id": 42, "is_bot": false, "first_name": "alice" },
                    "text": "the unique fingerprint string"
                }
            }]
        })))
        .up_to_n_times(1)
        .mount(&server).await;
    Mock::given(method("POST")).and(path("/bot12345:fake/GetUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "result": []
        })))
        .mount(&server).await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    let cfg = format!(r#"
[bot]
token = "12345:fake"
api_base_url = "{}"

[storage]
path = "{}"

[updater]
enabled = true
poll_timeout_secs = 1

[aliases]
"#, server.uri(), db_path.display().to_string().replace('\\', "/"));
    std::fs::write(&cfg_path, cfg).unwrap();

    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    // give updater a moment to consume the batch
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let resp = client.call_tool("tg_history_search", serde_json::json!({
        "query": "fingerprint"
    }));
    let content = &resp["result"]["content"][0]["text"];
    let parsed: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert!(parsed.as_array().unwrap().iter().any(|h| h["chat_id"] == 42));
}
```

- [ ] **Step 2: Run, confirm pass**

Run: `cargo test -p mcp-server --test smoke updater_enabled_flows_inbound_to_history_search`
Expected: PASS.

- [ ] **Step 3: Full workspace test sweep**

Run: `cargo test --workspace --all-targets`
Expected: all pass.

- [ ] **Step 4: Final lint sweep**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/mcp-server/tests/
git commit -m "test(mcp-server): e2e updater + history_search end-to-end flow"
```

---

## Milestone 8 — Docs & distribution

### Task 25: config.example.toml

**Files:**
- Create: `config.example.toml`

- [ ] **Step 1: Write the file**

```toml
# TelegramMCP example configuration.
# Copy to `config.toml` (which is gitignored) and edit.

[bot]
# Prefer the env var. The bot token is never logged.
token_env = "TELEGRAM_BOT_TOKEN"
# token = "123456:ABC..."          # inline alternative; warning logged
# api_base_url = "https://api.telegram.org"   # override for tests

[storage]
# SQLite history database. Parent directory must exist.
path = "C:/Users/user/AppData/Local/TelegramMCP/history.db"

[updater]
enabled = true
poll_timeout_secs = 30
allowed_update_kinds = [
  "message", "edited_message",
  "channel_post", "edited_channel_post",
]

# [retention]
# max_age_days = 365
# max_messages_total = 1000000

[aliases]
me = 12345678
alerts = -1001234567890
"team-eng" = -1009876543210

[access]
# Inbound: drop updates from chats not in this list. Empty = open.
allowed_chats = ["me", "alerts", "team-eng"]
# Outbound: refuse tg_send_* targeting chats not in this list. Empty = unrestricted.
allowed_send_targets = []
```

- [ ] **Step 2: Commit**

```bash
git add config.example.toml
git commit -m "docs: config.example.toml template"
```

### Task 26: README.md

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write `README.md`**

```markdown
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
- `mcp-server` — `rmcp`-backed binary that wires it all together

## Security model

- **stdio transport.** Whoever spawned the binary already has access.
- **Bot token.** Never logged. Source from env var. `config.toml` is
  gitignored; `config.example.toml` is the tracked template.
- **No MTProto / user-account API.** Only the Bot API (HTTPS).
- **Allowlists.** Optional `[access] allowed_chats` (inbound drop filter) and
  `allowed_send_targets` (outbound deny) keep blast radius small.

## License

MIT OR Apache-2.0.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: README"
```

### Task 27: CLAUDE.md

**Files:**
- Create: `CLAUDE.md`

- [ ] **Step 1: Write `CLAUDE.md`**

```markdown
# CLAUDE.md

This file provides guidance to Claude Code when working with this repository.

## Project

TelegramMCP is a Rust MCP server that exposes a Telegram Bot to LLM clients
over stdio. Bidirectional: outbound Bot API calls + a background long-poll
loop that captures incoming updates into a local SQLite history. Design in
[docs/superpowers/specs/2026-05-13-telegram-mcp-design.md](docs/superpowers/specs/2026-05-13-telegram-mcp-design.md);
implementation plan in [docs/superpowers/plans/2026-05-13-telegram-mcp.md](docs/superpowers/plans/2026-05-13-telegram-mcp.md).

Single binary:
- `TelegramMCP` — the MCP server. JSON-RPC over stdio.

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
```

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md project guide for future Claude sessions"
```

### Task 28: Final checks

- [ ] **Step 1: Full lint sweep**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 2: Full test sweep**

Run: `cargo test --workspace --all-targets`
Expected: all pass.

- [ ] **Step 3: Format**

Run: `cargo fmt --all -- --check`
Expected: no diff.

- [ ] **Step 4: Supply-chain audit**

Run: `cargo deny check`
Expected: pass. (If first run: `cargo install cargo-deny --locked`.)

- [ ] **Step 5: Release build**

Run: `cargo build --workspace --release`
Expected: `target/release/TelegramMCP.exe` produced.

- [ ] **Step 6: Commit (only if any incidental changes)**

```bash
git status
# only commit if anything changed
git add -A && git commit -m "chore: final lint + format pass"
```

---

## Out of scope for this plan (deferred)

These appear in the spec's "future work" section and should land as separate plans:

- `mcp-console` REPL client crate (mirrors sibling project's pattern).
- Multi-bot in one server process.
- Webhook transport (new `tg-webhook` crate parallel to `tg-updater`).
- MTProto / user-account API.
- Edit-history table (`message_revisions`).
- Inline keyboards / callback queries.
- Background retention sweep task that runs `trim_older_than` /
  `trim_per_chat_to` on a timer (the methods exist; the scheduler does not).
- Reproducible builds, signed releases.
