# Telegram Stop-Hook Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `tg-hook`, a Windows Rust binary wired as a Claude Code Stop hook that sends a Telegram wakeup message, blocks waiting for the user's Telegram reply (up to 60 min) or a Ctrl+C, then either feeds the reply back to Claude as a new turn or asks Claude to retry on timeout.

**Architecture:** New workspace crate `tg-hook` produces a single binary. The hook does **not** open the SQLite DB directly — it connects to the running `TelegramMCP` instance's named pipe (already exposed by `local-pipe::run_pipe_server`), authenticates with the per-instance token from the discovery file, runs a minimal MCP handshake, and uses the existing `tg_send_message` + `tg_history_messages` tools. Discovery match is by `CLAUDE_SESSION_ID` (preferred) or PPID equality. On Stop hook input from Claude Code, the hook prints `{"decision":"block","reason":"…"}` to stdout when it has a reply or a retry-prompt for Claude; `Ctrl+C` exits 0 without a `decision`, so Claude stops normally.

**Tech Stack:** Rust (stable, edition 2024), workspace pedantic clippy, `tokio` (current-thread + signal), `tokio::net::windows::named_pipe::ClientOptions`, `serde`/`serde_json`, `thiserror` per-crate + `anyhow` at the binary boundary, `tracing` to a per-PID log file (never stderr while blocking — stderr is reserved for the user-visible status line). Tests use the existing wiremock pattern and a `local-pipe`-backed fake MCP server.

**Spec:** This plan; see also [../../CLAUDE.md](../../CLAUDE.md) and [crates/local-pipe/src/](../../crates/local-pipe/src/).

**Window:** Windows-only (mirrors `local-pipe`'s `#![cfg(windows)]`).

---

## File structure produced by this plan

```
TelegramMCP/
├── Cargo.toml                          (workspace member added)
├── crates/
│   └── tg-hook/
│       ├── Cargo.toml
│       ├── README.md                   short usage + settings.json snippet
│       ├── src/
│       │   ├── main.rs                 binary entry, tokio runtime, top-level error mapping
│       │   ├── lib.rs                  pub mod re-exports for integration tests
│       │   ├── cli.rs                  HookArgs (clap-free parser)
│       │   ├── stop_input.rs           Claude Code Stop hook stdin model
│       │   ├── discovery.rs            pick the right DiscoveryRecord
│       │   ├── pipe_client.rs          connect + AUTH + framed line I/O
│       │   ├── mcp_client.rs           initialize + call_tool over PipeClient
│       │   ├── run.rs                  send → poll → reply/retry control flow
│       │   └── error.rs                HookError
│       └── tests/
│           ├── common/
│           │   └── mod.rs              FakeMcpPipe — spawns local-pipe server with canned tool responses
│           ├── discovery_selection.rs
│           ├── pipe_auth.rs
│           └── run_e2e.rs              full hook flow against FakeMcpPipe
└── docs/
    └── superpowers/plans/
        └── 2026-05-14-telegram-stop-hook.md   (this file)
```

A test-only helper `FakeMcpPipe` lives under `crates/tg-hook/tests/common/mod.rs` and is **not** re-exported from `tg-hook`'s public API. It uses the real `local-pipe::run_pipe_server` so the wire format under test is the production format.

---

## Decisions locked in

1. **Timeout:** 60 minutes per wait cycle (`--timeout-secs 3600`, configurable).
2. **Retry semantics:** on timeout, exit with `decision: block` and a `reason` instructing Claude to send a brief status; the hook re-fires on Claude's next Stop. **No internal "still waiting" auto-pings** — every ping is a real Claude turn, so the user sees genuine activity.
3. **Escape:** SIGINT / Ctrl+C from the parent Claude Code session terminates the hook child; the hook installs a `tokio::signal::ctrl_c()` handler that exits 0 with **no** `decision` field — Claude stops normally.
4. **Status line:** the hook writes one line to stderr immediately after the wakeup send succeeds:
   `Waiting on keyboard interrupt or external message (60 min)`
   Claude Code displays hook stderr to the user, which is what we want.
5. **Discovery match:** `CLAUDE_SESSION_ID` env var first (if both the hook env and the discovery record have it), else exact `ppid` match against the hook's own PPID. Multiple matches → newest `started_at` wins. Zero matches → hard error.
6. **Logs:** `tracing` writes to `%LOCALAPPDATA%\TelegramMCP\logs\tg-hook-<pid>.log`. Never to stderr (would pollute the status line) and never to stdout (would corrupt the JSON response).
7. **Polling cadence:** 2-second `tg_history_messages` polls. Cheap (local pipe + SQLite), bounded by 30 polls/min × 60 min = 1800 calls per cycle.
8. **CLI shape:** `tg-hook --chat <alias-or-id> --message <text> [--retry-message <text>] [--timeout-secs N] [--poll-secs N]`. No subcommands. Roll a tiny arg parser (consistent with the existing `parse_cli` in `mcp-server/src/main.rs`); avoid adding `clap` for one binary.

---

## Milestone 1 — Crate scaffold and Cargo wiring

### Task 1: Add the `tg-hook` crate to the workspace

**Files:**
- Create: `crates/tg-hook/Cargo.toml`
- Create: `crates/tg-hook/src/main.rs`
- Create: `crates/tg-hook/src/lib.rs`
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]` table — add `tg-hook = { path = "crates/tg-hook" }` line)

- [ ] **Step 1: Create the crate Cargo.toml**

```toml
[package]
name = "tg-hook"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
rust-version.workspace = true
publish = false

[lints]
workspace = true

[[bin]]
name = "tg-hook"
path = "src/main.rs"

[lib]
path = "src/lib.rs"

[dependencies]
local-pipe = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
dirs = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 2: Create a placeholder `src/lib.rs`** — exposes nothing yet, but lets integration tests under `tests/` link against the crate.

```rust
//! Library surface for the `tg-hook` binary, used by integration tests.
//!
//! Modules are added incrementally by later tasks.
#![cfg(windows)]
```

- [ ] **Step 3: Create a placeholder `src/main.rs` that compiles and exits 0**

```rust
//! `tg-hook` — Claude Code Stop hook that sends a Telegram message and
//! waits for a reply before letting Claude stop.
#![cfg(windows)]

fn main() {
    // Real entry wired in Task 9.
}
```

- [ ] **Step 4: Add the workspace dependency entry**

In `Cargo.toml`, inside `[workspace.dependencies]`, add **directly after** the existing `local-pipe = { path = "crates/local-pipe" }` line:

```toml
tg-hook = { path = "crates/tg-hook" }
```

(There is no `[workspace.members]` to edit — the workspace already uses `members = ["crates/*"]`, so dropping the directory in is enough.)

- [ ] **Step 5: Verify the workspace still builds**

Run: `cargo check --workspace`
Expected: clean exit, `tg-hook` listed among the compiled crates.

- [ ] **Step 6: Commit**

```powershell
git add Cargo.toml crates/tg-hook/
git commit -m "feat(tg-hook): add empty windows-only crate scaffold"
```

---

## Milestone 2 — Inputs: CLI args and Stop hook stdin

### Task 2: CLI arg parsing

**Files:**
- Create: `crates/tg-hook/src/cli.rs`
- Modify: `crates/tg-hook/src/lib.rs` (add `pub mod cli;`)

- [ ] **Step 1: Write the failing tests at `crates/tg-hook/src/cli.rs`**

```rust
//! Hand-rolled CLI arg parser for `tg-hook`.
//!
//! Avoids pulling `clap` for one binary; mirrors the style of
//! `mcp-server/src/main.rs::parse_cli`.

use thiserror::Error;

/// Parsed command-line arguments.
#[derive(Debug, PartialEq, Eq)]
pub struct HookArgs {
    /// Chat alias or numeric chat id to send the wakeup to.
    pub chat: String,
    /// Wakeup message text.
    pub message: String,
    /// Message returned to Claude on timeout (becomes the `reason` field).
    pub retry_message: String,
    /// Timeout per wait cycle, in seconds. Default 3600 (60 min).
    pub timeout_secs: u64,
    /// Poll interval, in seconds. Default 2.
    pub poll_secs: u64,
}

/// Parse errors for [`parse_args`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ArgsError {
    /// A required flag was not supplied.
    #[error("missing required argument: --{0}")]
    Missing(&'static str),
    /// A flag was supplied without its value.
    #[error("--{0} requires a value")]
    NeedsValue(String),
    /// `--timeout-secs` or `--poll-secs` was not a positive integer.
    #[error("--{name} must be a positive integer (got `{value}`)")]
    BadInt {
        /// Flag name.
        name: &'static str,
        /// Raw value.
        value: String,
    },
    /// An unknown flag was passed.
    #[error("unknown argument: {0}")]
    Unknown(String),
}

const DEFAULT_TIMEOUT: u64 = 3600;
const DEFAULT_POLL: u64 = 2;
const DEFAULT_RETRY_MSG: &str =
    "60 minutes elapsed without a Telegram reply. Send a brief status \
     update — the hook will rearm and keep listening.";

/// Parse `args` (NOT including argv[0]) into [`HookArgs`].
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<HookArgs, ArgsError> {
    let mut chat: Option<String> = None;
    let mut message: Option<String> = None;
    let mut retry: Option<String> = None;
    let mut timeout: u64 = DEFAULT_TIMEOUT;
    let mut poll: u64 = DEFAULT_POLL;

    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--chat" => chat = Some(it.next().ok_or_else(|| ArgsError::NeedsValue("chat".into()))?),
            "--message" => {
                message = Some(it.next().ok_or_else(|| ArgsError::NeedsValue("message".into()))?);
            }
            "--retry-message" => {
                retry = Some(it.next().ok_or_else(|| ArgsError::NeedsValue("retry-message".into()))?);
            }
            "--timeout-secs" => {
                let v = it.next().ok_or_else(|| ArgsError::NeedsValue("timeout-secs".into()))?;
                timeout = v.parse().map_err(|_| ArgsError::BadInt {
                    name: "timeout-secs",
                    value: v,
                })?;
            }
            "--poll-secs" => {
                let v = it.next().ok_or_else(|| ArgsError::NeedsValue("poll-secs".into()))?;
                poll = v.parse().map_err(|_| ArgsError::BadInt {
                    name: "poll-secs",
                    value: v,
                })?;
            }
            other => return Err(ArgsError::Unknown(other.to_string())),
        }
    }

    Ok(HookArgs {
        chat: chat.ok_or(ArgsError::Missing("chat"))?,
        message: message.ok_or(ArgsError::Missing("message"))?,
        retry_message: retry.unwrap_or_else(|| DEFAULT_RETRY_MSG.to_string()),
        timeout_secs: timeout,
        poll_secs: poll,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_minimal_required() {
        let got = parse_args(args(&["--chat", "alerts", "--message", "done"])).unwrap();
        assert_eq!(got.chat, "alerts");
        assert_eq!(got.message, "done");
        assert_eq!(got.timeout_secs, 3600);
        assert_eq!(got.poll_secs, 2);
        assert!(!got.retry_message.is_empty());
    }

    #[test]
    fn missing_chat_is_error() {
        let err = parse_args(args(&["--message", "x"])).unwrap_err();
        assert_eq!(err, ArgsError::Missing("chat"));
    }

    #[test]
    fn missing_message_is_error() {
        let err = parse_args(args(&["--chat", "alerts"])).unwrap_err();
        assert_eq!(err, ArgsError::Missing("message"));
    }

    #[test]
    fn unknown_flag_rejected() {
        let err = parse_args(args(&["--chat", "alerts", "--message", "x", "--banana"])).unwrap_err();
        assert!(matches!(err, ArgsError::Unknown(s) if s == "--banana"));
    }

    #[test]
    fn timeout_not_a_number() {
        let err = parse_args(args(&[
            "--chat", "alerts", "--message", "x", "--timeout-secs", "five",
        ]))
        .unwrap_err();
        assert!(matches!(err, ArgsError::BadInt { name: "timeout-secs", .. }));
    }

    #[test]
    fn custom_retry_message_overrides_default() {
        let got = parse_args(args(&[
            "--chat", "alerts", "--message", "x", "--retry-message", "still here?",
        ]))
        .unwrap();
        assert_eq!(got.retry_message, "still here?");
    }
}
```

- [ ] **Step 2: Run the tests, watch them fail because the module is not registered**

Run: `cargo test -p tg-hook --lib`
Expected: `cargo` reports `unresolved module \`cli\`` (or the test target does not exist yet because no items are wired).

- [ ] **Step 3: Register the module**

In `crates/tg-hook/src/lib.rs`, replace the file contents with:

```rust
//! Library surface for the `tg-hook` binary, used by integration tests.
#![cfg(windows)]

pub mod cli;
```

- [ ] **Step 4: Run the tests, watch them pass**

Run: `cargo test -p tg-hook --lib`
Expected: 5 passing tests in `cli::tests`.

- [ ] **Step 5: Commit**

```powershell
git add crates/tg-hook/src/cli.rs crates/tg-hook/src/lib.rs
git commit -m "feat(tg-hook): CLI arg parser with defaults"
```

### Task 3: Stop hook stdin model

**Files:**
- Create: `crates/tg-hook/src/stop_input.rs`
- Modify: `crates/tg-hook/src/lib.rs` (add `pub mod stop_input;`)

- [ ] **Step 1: Write the failing tests at `crates/tg-hook/src/stop_input.rs`**

```rust
//! JSON payload Claude Code feeds to a Stop hook on stdin.
//!
//! Only the fields we actually read are modeled. Unknown fields are
//! ignored (forward-compat with Claude Code stop-hook payload changes).

use serde::Deserialize;

/// Stop hook stdin payload. Both fields are optional in practice — older
/// Claude Code builds omit them — so we tolerate either being missing.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct StopInput {
    /// Claude Code session identifier. When present, prefer this over
    /// PPID for picking the right MCP server discovery record.
    #[serde(default)]
    pub session_id: Option<String>,
    /// True when this Stop hook fired *because* a previous Stop hook
    /// returned `decision: block`. We log this for observability but do
    /// not change behavior — every cycle is a fresh 60-min wait.
    #[serde(default)]
    pub stop_hook_active: bool,
}

/// Parse the Stop hook stdin JSON. An empty/whitespace-only input is
/// treated as `StopInput::default()` so the hook still runs when invoked
/// by hand (e.g. for testing) without a payload.
pub fn parse(stdin: &str) -> Result<StopInput, serde_json::Error> {
    let trimmed = stdin.trim();
    if trimmed.is_empty() {
        return Ok(StopInput::default());
    }
    serde_json::from_str(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_default() {
        let got = parse("").unwrap();
        assert_eq!(got, StopInput::default());
    }

    #[test]
    fn whitespace_only_yields_default() {
        let got = parse("   \n").unwrap();
        assert_eq!(got, StopInput::default());
    }

    #[test]
    fn parses_session_id_and_active_flag() {
        let s = r#"{"session_id":"abc-123","stop_hook_active":true}"#;
        let got = parse(s).unwrap();
        assert_eq!(got.session_id.as_deref(), Some("abc-123"));
        assert!(got.stop_hook_active);
    }

    #[test]
    fn ignores_unknown_fields() {
        let s = r#"{"session_id":"x","transcript_path":"/tmp/t","future_field":42}"#;
        let got = parse(s).unwrap();
        assert_eq!(got.session_id.as_deref(), Some("x"));
        assert!(!got.stop_hook_active);
    }
}
```

- [ ] **Step 2: Run, see it fail with `unresolved module`**

Run: `cargo test -p tg-hook --lib`
Expected: fail until next step.

- [ ] **Step 3: Register the module**

In `crates/tg-hook/src/lib.rs`, append:

```rust
pub mod stop_input;
```

- [ ] **Step 4: Run, see all tests green**

Run: `cargo test -p tg-hook --lib`
Expected: 9 passing tests (5 cli + 4 stop_input).

- [ ] **Step 5: Commit**

```powershell
git add crates/tg-hook/src/stop_input.rs crates/tg-hook/src/lib.rs
git commit -m "feat(tg-hook): parse Stop hook stdin payload"
```

---

## Milestone 3 — Discovery and pipe handshake

### Task 4: Pick the right MCP server discovery record

**Files:**
- Create: `crates/tg-hook/src/discovery.rs`
- Create: `crates/tg-hook/src/error.rs`
- Modify: `crates/tg-hook/src/lib.rs`

- [ ] **Step 1: Write the error enum at `crates/tg-hook/src/error.rs`**

```rust
//! Unified error type for `tg-hook`.

use thiserror::Error;

/// Errors surfaced by the hook's library modules.
#[derive(Debug, Error)]
pub enum HookError {
    /// I/O error reading the discovery directory or a discovery file.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// Discovery directory exists but contains no matching record.
    #[error("no TelegramMCP discovery record matches this Claude Code session \
             (looked for session_id={session:?}, ppid={ppid})")]
    NoMatchingServer {
        /// CLAUDE_SESSION_ID at hook invocation, if any.
        session: Option<String>,
        /// Hook's own PPID at invocation.
        ppid: u32,
    },
    /// A discovery file was found but its JSON failed to parse.
    #[error("malformed discovery file {path}: {source}")]
    BadDiscovery {
        /// Discovery file path.
        path: String,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
}
```

- [ ] **Step 2: Register the module**

In `crates/tg-hook/src/lib.rs`, append:

```rust
pub mod error;
pub mod discovery;
```

- [ ] **Step 3: Write the failing tests + impl at `crates/tg-hook/src/discovery.rs`**

```rust
//! Discovery-file selection logic.
//!
//! The MCP server writes a per-PID JSON record under
//! `%LOCALAPPDATA%\TelegramMCP\discovery\` (see [`local_pipe::discovery`]).
//! The hook scans that directory, filters to records whose `session_id`
//! matches `CLAUDE_SESSION_ID` (preferred) or whose `ppid` equals the
//! hook's own PPID, and picks the newest `started_at` on ties.

use crate::error::HookError;
use local_pipe::DiscoveryRecord;
use std::path::Path;

/// Read all discovery records from `dir`, ignoring malformed files
/// (logged via `tracing`). Returns the records and the paths they came
/// from, so callers can include the path in error messages.
fn read_all(dir: &Path) -> Result<Vec<(DiscoveryRecord, String)>, HookError> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(HookError::Io(e)),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, path = %path.display(), "discovery read failed");
                continue;
            }
        };
        match serde_json::from_str::<DiscoveryRecord>(&raw) {
            Ok(rec) => out.push((rec, path.display().to_string())),
            Err(e) => {
                tracing::debug!(error = %e, path = %path.display(), "discovery parse failed");
            }
        }
    }
    Ok(out)
}

/// Select the best match for this hook invocation.
///
/// Preference order:
/// 1. `session_id` exact match (when `session` is `Some` and the record
///    also has a `session_id`).
/// 2. `ppid` exact match.
/// Within either bucket, newest `started_at` (lexicographic ISO-8601
/// ordering is correct for the format `local-pipe` emits) wins.
pub fn pick(
    dir: &Path,
    session: Option<&str>,
    ppid: u32,
) -> Result<DiscoveryRecord, HookError> {
    let records = read_all(dir)?;

    let mut session_matches: Vec<&DiscoveryRecord> = Vec::new();
    let mut ppid_matches: Vec<&DiscoveryRecord> = Vec::new();
    for (rec, _) in &records {
        if let (Some(want), Some(got)) = (session, rec.session_id.as_deref()) {
            if want == got {
                session_matches.push(rec);
                continue;
            }
        }
        if rec.ppid == ppid {
            ppid_matches.push(rec);
        }
    }

    let pool = if !session_matches.is_empty() {
        session_matches
    } else {
        ppid_matches
    };
    pool.into_iter()
        .max_by(|a, b| a.started_at.cmp(&b.started_at))
        .cloned()
        .ok_or(HookError::NoMatchingServer {
            session: session.map(str::to_string),
            ppid,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_record(dir: &Path, pid: u32, ppid: u32, session: Option<&str>, started_at: &str) {
        let rec = serde_json::json!({
            "pid": pid,
            "ppid": ppid,
            "pipe": format!(r"\\.\pipe\telegrammcp-{pid}"),
            "token": "tok",
            "session_id": session,
            "started_at": started_at,
        });
        std::fs::write(
            dir.join(format!("{pid}.json")),
            serde_json::to_string_pretty(&rec).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn picks_session_match_over_ppid_match() {
        let d = tempdir().unwrap();
        // record A: ppid match, no session
        write_record(d.path(), 100, 50, None, "2026-05-14T00:00:00Z");
        // record B: session match, ppid mismatch
        write_record(d.path(), 200, 999, Some("S"), "2026-05-13T00:00:00Z");
        let got = pick(d.path(), Some("S"), 50).unwrap();
        assert_eq!(got.pid, 200, "session match should outrank ppid match");
    }

    #[test]
    fn picks_newest_ppid_match_when_no_session() {
        let d = tempdir().unwrap();
        write_record(d.path(), 100, 50, None, "2026-05-14T00:00:00Z");
        write_record(d.path(), 101, 50, None, "2026-05-14T01:00:00Z");
        let got = pick(d.path(), None, 50).unwrap();
        assert_eq!(got.pid, 101);
    }

    #[test]
    fn no_match_is_an_error() {
        let d = tempdir().unwrap();
        write_record(d.path(), 100, 999, None, "2026-05-14T00:00:00Z");
        let err = pick(d.path(), None, 50).unwrap_err();
        assert!(matches!(err, HookError::NoMatchingServer { .. }));
    }

    #[test]
    fn missing_directory_is_no_match() {
        let d = tempdir().unwrap();
        let missing = d.path().join("does-not-exist");
        let err = pick(&missing, None, 50).unwrap_err();
        assert!(matches!(err, HookError::NoMatchingServer { .. }));
    }

    #[test]
    fn ignores_non_json_files() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("README.txt"), "ignore me").unwrap();
        write_record(d.path(), 100, 50, None, "2026-05-14T00:00:00Z");
        let got = pick(d.path(), None, 50).unwrap();
        assert_eq!(got.pid, 100);
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p tg-hook --lib discovery`
Expected: 5 passing tests.

- [ ] **Step 5: Commit**

```powershell
git add crates/tg-hook/src/discovery.rs crates/tg-hook/src/error.rs crates/tg-hook/src/lib.rs
git commit -m "feat(tg-hook): select matching MCP discovery record"
```

### Task 5: Pipe client — connect, AUTH, framed line I/O

**Files:**
- Create: `crates/tg-hook/src/pipe_client.rs`
- Modify: `crates/tg-hook/src/error.rs` (add Pipe variants)
- Modify: `crates/tg-hook/src/lib.rs` (add `pub mod pipe_client;`)

- [ ] **Step 1: Extend `HookError` with pipe variants**

In `crates/tg-hook/src/error.rs`, add new variants **before** the closing `}`:

```rust
    /// Failed to connect to the MCP server's named pipe.
    #[error("connecting to pipe {pipe}: {source}")]
    PipeConnect {
        /// Pipe path that was being opened.
        pipe: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// MCP server closed the pipe before responding.
    #[error("MCP server closed the pipe unexpectedly")]
    PipeClosed,
```

- [ ] **Step 2: Write the impl + a unit test that exercises the live `local-pipe` server via a small harness**

Create `crates/tg-hook/src/pipe_client.rs`:

```rust
//! Async line-framed I/O over the MCP server's named pipe.
//!
//! Wraps a `NamedPipeClient` to:
//!   1. send `AUTH <token>\n` as the very first bytes;
//!   2. expose `send_line` / `recv_line` for the JSON-RPC layer above.
//!
//! No knowledge of MCP semantics lives here — that's [`crate::mcp_client`].

use crate::error::HookError;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

/// Authenticated bidirectional line stream over a TelegramMCP pipe.
pub struct PipeClient {
    /// Buffered reader; owns the read half of the pipe.
    reader: BufReader<tokio::io::ReadHalf<NamedPipeClient>>,
    /// Write half of the pipe.
    writer: tokio::io::WriteHalf<NamedPipeClient>,
}

impl PipeClient {
    /// Open `pipe_path`, send `AUTH <token>\n`, and return a usable client.
    ///
    /// Connect retries for up to 2 seconds with a 50ms cadence — the
    /// server creates a fresh listener instance after each accept, so
    /// it's normal to race the next instance.
    pub async fn connect(pipe_path: &str, token: &str) -> Result<Self, HookError> {
        let pipe = open_with_retry(pipe_path).await?;
        let (r, mut w) = tokio::io::split(pipe);
        // AUTH line first — see local-pipe/src/auth.rs.
        w.write_all(format!("AUTH {token}\n").as_bytes())
            .await
            .map_err(|source| HookError::PipeConnect {
                pipe: pipe_path.to_string(),
                source,
            })?;
        w.flush().await.map_err(|source| HookError::PipeConnect {
            pipe: pipe_path.to_string(),
            source,
        })?;
        Ok(Self {
            reader: BufReader::new(r),
            writer: w,
        })
    }

    /// Send one JSON-RPC line, terminated with `\n`.
    pub async fn send_line(&mut self, line: &str) -> Result<(), HookError> {
        debug_assert!(!line.contains('\n'), "send_line received a multi-line payload");
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Read one line (without the terminating `\n`). Returns
    /// [`HookError::PipeClosed`] on clean EOF.
    pub async fn recv_line(&mut self) -> Result<String, HookError> {
        let mut buf = String::new();
        let n = self.reader.read_line(&mut buf).await?;
        if n == 0 {
            return Err(HookError::PipeClosed);
        }
        if buf.ends_with('\n') {
            buf.pop();
        }
        if buf.ends_with('\r') {
            buf.pop();
        }
        Ok(buf)
    }
}

async fn open_with_retry(pipe_path: &str) -> Result<NamedPipeClient, HookError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut last_err: Option<std::io::Error> = None;
    while std::time::Instant::now() < deadline {
        match ClientOptions::new().open(pipe_path) {
            Ok(c) => return Ok(c),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    Err(HookError::PipeConnect {
        pipe: pipe_path.to_string(),
        source: last_err.unwrap_or_else(|| std::io::Error::other("open retry exhausted")),
    })
}
```

- [ ] **Step 3: Register the module**

In `crates/tg-hook/src/lib.rs`, append:

```rust
pub mod pipe_client;
```

- [ ] **Step 4: Write the integration test against a real `local-pipe` server**

Create `crates/tg-hook/tests/pipe_auth.rs`:

```rust
//! Integration test: PipeClient against the real `local-pipe::run_pipe_server`.
//!
//! Exercises the AUTH handshake and line-framed echo.
#![cfg(windows)]

use local_pipe::run_pipe_server;
use std::sync::Arc;
use tg_hook::pipe_client::PipeClient;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test(flavor = "current_thread")]
async fn auth_handshake_then_echo() {
    let handler: local_pipe::ConnHandler = Arc::new(|pipe| {
        Box::pin(async move {
            let (r, mut w) = tokio::io::split(pipe);
            let mut reader = BufReader::new(r);
            let mut buf = String::new();
            // Echo each received line back, prefixed.
            while reader.read_line(&mut buf).await.unwrap_or(0) > 0 {
                let trimmed = buf.trim_end().to_string();
                let _ = w.write_all(format!("echo: {trimmed}\n").as_bytes()).await;
                let _ = w.flush().await;
                buf.clear();
            }
        })
    });
    let server_task = tokio::spawn(run_pipe_server(handler));
    // give the listener a moment to bind
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Look up *this* test process's discovery record.
    let pid = std::process::id();
    let path = local_pipe::discovery::discovery_file_for(pid).unwrap();
    let raw = std::fs::read_to_string(&path).expect("discovery file");
    let rec: local_pipe::DiscoveryRecord = serde_json::from_str(&raw).unwrap();

    let mut client = PipeClient::connect(&rec.pipe, &rec.token).await.expect("connect");
    client.send_line("hello").await.expect("send");
    let got = client.recv_line().await.expect("recv");
    assert_eq!(got, "echo: hello");

    server_task.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn bad_token_closes_pipe() {
    let handler: local_pipe::ConnHandler = Arc::new(|_pipe| Box::pin(async move {}));
    let server_task = tokio::spawn(run_pipe_server(handler));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let pid = std::process::id();
    let path = local_pipe::discovery::discovery_file_for(pid).unwrap();
    let raw = std::fs::read_to_string(&path).expect("discovery file");
    let rec: local_pipe::DiscoveryRecord = serde_json::from_str(&raw).unwrap();

    let mut client = PipeClient::connect(&rec.pipe, "wrong-token").await.expect("connect");
    // Server drops the connection on bad token; the next recv_line should
    // surface PipeClosed (or an Io error wrapping it).
    let err = client.recv_line().await.unwrap_err();
    match err {
        tg_hook::error::HookError::PipeClosed | tg_hook::error::HookError::Io(_) => {}
        other => panic!("unexpected error variant: {other:?}"),
    }

    server_task.abort();
}
```

> **Note:** `local_pipe::discovery::discovery_file_for` and `local_pipe::discovery::discovery_dir` are not currently re-exported from `local_pipe`'s root. The smoke test relies on them; if `pub use` is missing, add `pub use discovery;` to `crates/local-pipe/src/lib.rs` as a tiny prerequisite step (the module is already `pub mod`, so this just shortens callsites). The test already imports them via `local_pipe::discovery::...` which works either way.

- [ ] **Step 5: Run**

Run: `cargo test -p tg-hook --test pipe_auth`
Expected: 2 passing tests.

- [ ] **Step 6: Commit**

```powershell
git add crates/tg-hook/src/pipe_client.rs crates/tg-hook/src/error.rs crates/tg-hook/src/lib.rs crates/tg-hook/tests/pipe_auth.rs
git commit -m "feat(tg-hook): authenticated line-framed pipe client"
```

---

## Milestone 4 — Minimal MCP client

### Task 6: `initialize` + `tools/call` over a `PipeClient`

**Files:**
- Create: `crates/tg-hook/src/mcp_client.rs`
- Modify: `crates/tg-hook/src/error.rs` (add MCP variants)
- Modify: `crates/tg-hook/src/lib.rs`

- [ ] **Step 1: Extend `HookError`**

In `crates/tg-hook/src/error.rs`, before the closing `}`, add:

```rust
    /// JSON-RPC error returned by the MCP server.
    #[error("MCP error from server (method={method}): {message}")]
    Rpc {
        /// Method that errored.
        method: String,
        /// Error message field.
        message: String,
    },
    /// Failed to parse a JSON-RPC response.
    #[error("malformed MCP response (method={method}): {source}")]
    BadRpcResponse {
        /// Method whose response failed to parse.
        method: String,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
```

- [ ] **Step 2: Write the impl at `crates/tg-hook/src/mcp_client.rs`**

```rust
//! Minimal MCP JSON-RPC client over a [`crate::pipe_client::PipeClient`].
//!
//! Implements only what the hook needs: `initialize` + `tools/call` + the
//! `notifications/initialized` follow-up. Each request gets a monotonic id;
//! incoming lines whose id does not match the outstanding request are
//! dropped (notifications/server-initiated traffic — not expected today,
//! but allowed by MCP).

use crate::error::HookError;
use crate::pipe_client::PipeClient;
use serde_json::{Value, json};

/// Thin MCP client.
pub struct McpClient {
    pipe: PipeClient,
    next_id: u64,
}

impl McpClient {
    /// Wrap a [`PipeClient`] that has already been authenticated.
    pub fn new(pipe: PipeClient) -> Self {
        Self { pipe, next_id: 1 }
    }

    /// Drive the MCP `initialize` request and the matching
    /// `notifications/initialized` follow-up. Returns the server's
    /// `result` payload (mostly for tests).
    pub async fn initialize(&mut self) -> Result<Value, HookError> {
        let resp = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "tg-hook", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;
        // Required by MCP: send the initialized notification (no id).
        let notify = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.pipe.send_line(&notify.to_string()).await?;
        Ok(resp)
    }

    /// Call a tool by name, returning the raw `result.content[0].text`
    /// payload parsed as JSON. The hook's tool calls always return a
    /// single text content block whose body is JSON.
    pub async fn call_tool(&mut self, name: &str, args: Value) -> Result<Value, HookError> {
        let resp = self
            .request("tools/call", json!({ "name": name, "arguments": args }))
            .await?;
        let text = resp
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| HookError::BadRpcResponse {
                method: format!("tools/call:{name}"),
                source: serde::de::Error::custom("missing content[0].text"),
            })?;
        serde_json::from_str(text).map_err(|source| HookError::BadRpcResponse {
            method: format!("tools/call:{name}"),
            source,
        })
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, HookError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.pipe.send_line(&req.to_string()).await?;

        loop {
            let line = self.pipe.recv_line().await?;
            let v: Value = serde_json::from_str(&line).map_err(|source| {
                HookError::BadRpcResponse {
                    method: method.to_string(),
                    source,
                }
            })?;
            // Ignore notifications (no id) and any out-of-order id.
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                return Err(HookError::Rpc {
                    method: method.to_string(),
                    message,
                });
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}
```

- [ ] **Step 3: Register the module**

In `crates/tg-hook/src/lib.rs`, append:

```rust
pub mod mcp_client;
```

- [ ] **Step 4: Test against `FakeMcpPipe` — first define the helper**

Create `crates/tg-hook/tests/common/mod.rs`:

```rust
//! Test helper: a tiny `local-pipe` server that speaks just enough MCP
//! to drive the hook's `McpClient`. Each test registers tool responses
//! via the closure passed to [`FakeMcpPipe::spawn`].
#![cfg(windows)]
#![allow(dead_code, reason = "shared across integration tests")]

use local_pipe::{run_pipe_server, DiscoveryRecord};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

/// Function called per `tools/call` request. Receives the tool name and
/// the `arguments` value, returns the JSON the tool should output (as if
/// it were `result.content[0].text` parsed).
pub type ToolHandler = Arc<dyn Fn(&str, &Value) -> Value + Send + Sync>;

/// Spawned fake server. Drop or `.abort()` to tear down.
pub struct FakeMcpPipe {
    pub task: JoinHandle<Result<(), local_pipe::PipeError>>,
}

impl FakeMcpPipe {
    pub async fn spawn(handler: ToolHandler) -> Self {
        let conn_handler: local_pipe::ConnHandler = Arc::new(move |pipe| {
            let h = handler.clone();
            Box::pin(async move {
                let (r, mut w) = tokio::io::split(pipe);
                let mut reader = BufReader::new(r);
                let mut buf = String::new();
                while reader.read_line(&mut buf).await.unwrap_or(0) > 0 {
                    let line = buf.trim_end().to_string();
                    buf.clear();
                    let req: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let id = req.get("id").cloned();
                    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
                    if id.is_none() {
                        // notification — drop
                        continue;
                    }
                    let result = match method {
                        "initialize" => json!({
                            "protocolVersion": "2024-11-05",
                            "capabilities": {},
                            "serverInfo": {"name": "fake", "version": "0"}
                        }),
                        "tools/call" => {
                            let name = req
                                .pointer("/params/name")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let args = req.pointer("/params/arguments").cloned().unwrap_or(json!({}));
                            let payload = h(name, &args);
                            json!({
                                "content": [{
                                    "type": "text",
                                    "text": payload.to_string(),
                                }]
                            })
                        }
                        _ => json!(null),
                    };
                    let resp = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result,
                    });
                    let _ = w.write_all(format!("{resp}\n").as_bytes()).await;
                    let _ = w.flush().await;
                }
            })
        });
        let task = tokio::spawn(run_pipe_server(conn_handler));
        // Wait for the discovery file to appear.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        Self { task }
    }

    /// Read this process's own discovery record.
    pub fn discovery() -> DiscoveryRecord {
        let pid = std::process::id();
        let path = local_pipe::discovery::discovery_file_for(pid).unwrap();
        let raw = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&raw).unwrap()
    }
}

impl Drop for FakeMcpPipe {
    fn drop(&mut self) {
        self.task.abort();
    }
}
```

- [ ] **Step 5: Write the integration test exercising `McpClient`**

Append to `crates/tg-hook/tests/pipe_auth.rs`:

```rust
mod common;
use common::FakeMcpPipe;
use tg_hook::mcp_client::McpClient;

#[tokio::test(flavor = "current_thread")]
async fn initialize_and_call_tool() {
    let handler: common::ToolHandler = std::sync::Arc::new(|name, _args| {
        if name == "tg_send_message" {
            serde_json::json!({"chat_id": 42, "message_id": 7, "date": 1700})
        } else {
            serde_json::json!({})
        }
    });
    let _fake = FakeMcpPipe::spawn(handler).await;
    let rec = FakeMcpPipe::discovery();

    let pipe = tg_hook::pipe_client::PipeClient::connect(&rec.pipe, &rec.token)
        .await
        .expect("connect");
    let mut client = McpClient::new(pipe);
    client.initialize().await.expect("init");
    let out = client
        .call_tool(
            "tg_send_message",
            serde_json::json!({"chat": "alerts", "text": "hi"}),
        )
        .await
        .expect("call");
    assert_eq!(out["message_id"], 7);
    assert_eq!(out["chat_id"], 42);
}
```

- [ ] **Step 6: Run**

Run: `cargo test -p tg-hook --test pipe_auth`
Expected: 3 passing tests (the new `initialize_and_call_tool` plus the two from Task 5).

- [ ] **Step 7: Commit**

```powershell
git add crates/tg-hook/src/mcp_client.rs crates/tg-hook/src/error.rs crates/tg-hook/src/lib.rs crates/tg-hook/tests/common/ crates/tg-hook/tests/pipe_auth.rs
git commit -m "feat(tg-hook): minimal MCP client (initialize + tools/call)"
```

---

## Milestone 5 — Control flow

### Task 7: `run` — send → poll → reply/timeout/Ctrl+C

**Files:**
- Create: `crates/tg-hook/src/run.rs`
- Modify: `crates/tg-hook/src/lib.rs`

- [ ] **Step 1: Write the impl at `crates/tg-hook/src/run.rs`**

```rust
//! Top-level control flow for the hook.
//!
//! ```text
//! send wakeup ──► record baseline message_id
//!              ──► loop:
//!                    sleep(poll_secs)
//!                    fetch history(chat, after=baseline)
//!                    if inbound found → return Outcome::Reply
//!                    if elapsed >= timeout → return Outcome::RetryTimeout
//!                    if ctrl_c          → return Outcome::Interrupted
//! ```

use crate::cli::HookArgs;
use crate::error::HookError;
use crate::mcp_client::McpClient;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// What the hook decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// User replied via Telegram. Contains the reply text.
    Reply(String),
    /// `--timeout-secs` elapsed with no reply. Caller should return the
    /// configured retry message to Claude.
    RetryTimeout,
    /// Ctrl+C from the user. Caller should exit 0 without `decision`.
    Interrupted,
}

/// Run one full hook cycle.
///
/// `interrupt` is a future that resolves when the user wants to cancel
/// (production: `tokio::signal::ctrl_c()`; tests: a `oneshot::Receiver`).
pub async fn run<F>(
    args: &HookArgs,
    client: &mut McpClient,
    interrupt: F,
) -> Result<Outcome, HookError>
where
    F: std::future::Future<Output = ()> + Send,
{
    // 1. Send wakeup.
    let send_resp = client
        .call_tool(
            "tg_send_message",
            json!({ "chat": args.chat, "text": args.message }),
        )
        .await?;
    let chat_id = send_resp
        .get("chat_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HookError::BadRpcResponse {
            method: "tools/call:tg_send_message".into(),
            source: serde::de::Error::custom("missing chat_id"),
        })?;
    let baseline = send_resp
        .get("message_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HookError::BadRpcResponse {
            method: "tools/call:tg_send_message".into(),
            source: serde::de::Error::custom("missing message_id"),
        })?;

    // 2. Status line to stderr. Claude Code surfaces this to the user.
    //    Direct stderr write (not tracing) — tracing goes to the log file.
    eprintln!(
        "Waiting on keyboard interrupt or external message ({} min)",
        args.timeout_secs / 60
    );

    // 3. Poll loop.
    let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);
    let poll = Duration::from_secs(args.poll_secs.max(1));
    tokio::pin!(interrupt);
    loop {
        let sleep = tokio::time::sleep(poll);
        tokio::pin!(sleep);
        tokio::select! {
            biased; // interrupt wins over a ready sleep
            () = &mut interrupt => return Ok(Outcome::Interrupted),
            () = &mut sleep => {}
        }

        // Check for a reply newer than baseline. `tg_history_messages` is
        // newest-first and bounded; limit 50 is plenty since we only care
        // about messages strictly greater than `baseline` in this chat.
        let resp = client
            .call_tool(
                "tg_history_messages",
                json!({
                    "chat": args.chat,
                    "after_message_id": baseline,
                    "limit": 50
                }),
            )
            .await?;
        if let Some(text) = pick_inbound_reply(&resp, chat_id) {
            return Ok(Outcome::Reply(text));
        }

        if Instant::now() >= deadline {
            return Ok(Outcome::RetryTimeout);
        }
    }
}

/// From a `tg_history_messages` response (`Vec<StoredMessage>` JSON),
/// return the oldest inbound message's text, if any. Oldest-first so the
/// hook surfaces the *first* thing the user said.
fn pick_inbound_reply(history: &Value, chat_id: i64) -> Option<String> {
    let arr = history.as_array()?;
    let mut inbound: Vec<&Value> = arr
        .iter()
        .filter(|m| m.get("direction").and_then(Value::as_str) == Some("in"))
        .filter(|m| m.get("chat_id").and_then(Value::as_i64) == Some(chat_id))
        .collect();
    // history is newest-first; reverse to oldest-first.
    inbound.sort_by_key(|m| m.get("message_id").and_then(Value::as_i64).unwrap_or(0));
    inbound
        .first()
        .and_then(|m| m.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pick_inbound_skips_outbound() {
        let h = json!([
            {"chat_id": 1, "message_id": 11, "direction": "out", "text": "hi"},
            {"chat_id": 1, "message_id": 12, "direction": "in", "text": "yo"},
        ]);
        assert_eq!(pick_inbound_reply(&h, 1).as_deref(), Some("yo"));
    }

    #[test]
    fn pick_inbound_filters_wrong_chat() {
        let h = json!([
            {"chat_id": 99, "message_id": 12, "direction": "in", "text": "wrong"},
        ]);
        assert_eq!(pick_inbound_reply(&h, 1), None);
    }

    #[test]
    fn pick_inbound_picks_oldest_first() {
        let h = json!([
            {"chat_id": 1, "message_id": 20, "direction": "in", "text": "newer"},
            {"chat_id": 1, "message_id": 12, "direction": "in", "text": "first"},
        ]);
        assert_eq!(pick_inbound_reply(&h, 1).as_deref(), Some("first"));
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/tg-hook/src/lib.rs`, append:

```rust
pub mod run;
```

- [ ] **Step 3: Run unit tests**

Run: `cargo test -p tg-hook --lib run`
Expected: 3 passing tests.

- [ ] **Step 4: Write an e2e integration test driving the full flow against `FakeMcpPipe`**

Create `crates/tg-hook/tests/run_e2e.rs`:

```rust
#![cfg(windows)]

mod common;
use common::{FakeMcpPipe, ToolHandler};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tg_hook::cli::HookArgs;
use tg_hook::mcp_client::McpClient;
use tg_hook::pipe_client::PipeClient;
use tg_hook::run::{run, Outcome};

fn args(timeout: u64) -> HookArgs {
    HookArgs {
        chat: "alerts".into(),
        message: "done".into(),
        retry_message: "retry".into(),
        timeout_secs: timeout,
        poll_secs: 1,
    }
}

async fn connect(rec: &local_pipe::DiscoveryRecord) -> McpClient {
    let pipe = PipeClient::connect(&rec.pipe, &rec.token).await.unwrap();
    let mut c = McpClient::new(pipe);
    c.initialize().await.unwrap();
    c
}

#[tokio::test(flavor = "current_thread")]
async fn returns_reply_when_inbound_arrives() {
    // After the wakeup, return one inbound message on the very first poll.
    let polls = Arc::new(Mutex::new(0u32));
    let polls_for_h = polls.clone();
    let handler: ToolHandler = Arc::new(move |name, _args| match name {
        "tg_send_message" => json!({"chat_id": 1, "message_id": 100, "date": 1700}),
        "tg_history_messages" => {
            let mut n = polls_for_h.lock().unwrap();
            *n += 1;
            if *n >= 1 {
                json!([{
                    "chat_id": 1, "message_id": 101, "date": 1701,
                    "direction": "in", "text": "yo claude"
                }])
            } else {
                json!([])
            }
        }
        _ => json!({}),
    });
    let _fake = FakeMcpPipe::spawn(handler).await;
    let rec = FakeMcpPipe::discovery();
    let mut client = connect(&rec).await;
    let never_interrupt = std::future::pending::<()>();
    let outcome = run(&args(60), &mut client, never_interrupt).await.unwrap();
    assert_eq!(outcome, Outcome::Reply("yo claude".into()));
}

#[tokio::test(flavor = "current_thread")]
async fn times_out_when_no_reply() {
    let handler: ToolHandler = Arc::new(|name, _args| match name {
        "tg_send_message" => json!({"chat_id": 1, "message_id": 100, "date": 1700}),
        "tg_history_messages" => json!([]),
        _ => json!({}),
    });
    let _fake = FakeMcpPipe::spawn(handler).await;
    let rec = FakeMcpPipe::discovery();
    let mut client = connect(&rec).await;
    // timeout 2s, poll 1s → at most two polls before retry.
    let never_interrupt = std::future::pending::<()>();
    let mut a = args(2);
    a.poll_secs = 1;
    let outcome = run(&a, &mut client, never_interrupt).await.unwrap();
    assert_eq!(outcome, Outcome::RetryTimeout);
}

#[tokio::test(flavor = "current_thread")]
async fn interrupt_cancels_wait() {
    let handler: ToolHandler = Arc::new(|name, _args| match name {
        "tg_send_message" => json!({"chat_id": 1, "message_id": 100, "date": 1700}),
        "tg_history_messages" => json!([]),
        _ => json!({}),
    });
    let _fake = FakeMcpPipe::spawn(handler).await;
    let rec = FakeMcpPipe::discovery();
    let mut client = connect(&rec).await;
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let interrupt = async move {
        let _ = rx.await;
    };
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = tx.send(());
    });
    let outcome = run(&args(60), &mut client, interrupt).await.unwrap();
    assert_eq!(outcome, Outcome::Interrupted);
}
```

- [ ] **Step 5: Run**

Run: `cargo test -p tg-hook --test run_e2e -- --test-threads=1`
Expected: 3 passing tests. `--test-threads=1` because each test spawns a `local-pipe` server keyed on the current PID — only one server per PID at a time.

- [ ] **Step 6: Commit**

```powershell
git add crates/tg-hook/src/run.rs crates/tg-hook/src/lib.rs crates/tg-hook/tests/run_e2e.rs
git commit -m "feat(tg-hook): control loop with reply/timeout/interrupt outcomes"
```

---

## Milestone 6 — Wire it up

### Task 8: Binary entry point with logging, Ctrl+C, and JSON response

**Files:**
- Modify: `crates/tg-hook/src/main.rs` (replace placeholder with the full entry)

- [ ] **Step 1: Replace `crates/tg-hook/src/main.rs` with the real entry**

```rust
//! `tg-hook` — Claude Code Stop hook for TelegramMCP.
//!
//! Reads the Stop hook stdin payload + CLI args, finds the running
//! TelegramMCP instance via its discovery file, sends a wakeup message
//! over Telegram, waits up to `--timeout-secs` for a reply (polling
//! local history via the same pipe), and prints a Claude-Code-shaped
//! JSON response to stdout:
//!
//!   * Reply received  → `{"decision":"block","reason":"User replied via Telegram: <text>"}`
//!   * Timeout         → `{"decision":"block","reason":"<--retry-message>"}`
//!   * Ctrl+C          → exit 0 with **no** stdout (Claude stops normally)
//!
//! All `tracing` output goes to `%LOCALAPPDATA%\TelegramMCP\logs\tg-hook-<pid>.log`,
//! never to stderr or stdout — those are reserved for the status line and
//! the Claude response respectively.
#![cfg(windows)]

use anyhow::{Context, Result};
use serde_json::json;
use std::io::Read;
use tg_hook::cli::{HookArgs, parse_args};
use tg_hook::discovery;
use tg_hook::mcp_client::McpClient;
use tg_hook::pipe_client::PipeClient;
use tg_hook::run::{Outcome, run};
use tg_hook::stop_input;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    init_logging();

    let args: HookArgs = parse_args(std::env::args().skip(1))
        .context("parsing CLI arguments")?;

    let mut stdin_buf = String::new();
    std::io::stdin().read_to_string(&mut stdin_buf).ok();
    let stop = stop_input::parse(&stdin_buf).unwrap_or_default();
    tracing::info!(?stop, "hook started");

    // Discover the running MCP server.
    let dir = local_pipe::discovery::discovery_dir().context("discovery dir")?;
    let session_env = std::env::var("CLAUDE_SESSION_ID").ok();
    let session = stop.session_id.as_deref().or(session_env.as_deref());
    let ppid = parent_pid();
    let rec = discovery::pick(&dir, session, ppid).context("picking MCP server")?;
    tracing::info!(pid = rec.pid, ppid = rec.ppid, "selected MCP server");

    // Pipe + MCP handshake.
    let pipe = PipeClient::connect(&rec.pipe, &rec.token)
        .await
        .context("connecting to pipe")?;
    let mut client = McpClient::new(pipe);
    client.initialize().await.context("MCP initialize")?;

    // Wait for either a reply, a timeout, or Ctrl+C.
    let outcome = run(&args, &mut client, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .context("hook run loop")?;

    match outcome {
        Outcome::Reply(text) => {
            let resp = json!({
                "decision": "block",
                "reason": format!("User replied via Telegram: {text}"),
            });
            println!("{resp}");
        }
        Outcome::RetryTimeout => {
            let resp = json!({
                "decision": "block",
                "reason": args.retry_message,
            });
            println!("{resp}");
        }
        Outcome::Interrupted => {
            tracing::info!("hook interrupted by user; Claude will stop normally");
            // Print nothing — Claude Code interprets no decision as "stop ok".
        }
    }
    Ok(())
}

fn init_logging() {
    let log_file = dirs::data_local_dir().and_then(|base| {
        let dir = base.join("TelegramMCP").join("logs");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("tg-hook-{}.log", std::process::id()));
        std::fs::File::create(path).ok()
    });
    let filter = EnvFilter::try_from_env("TG_HOOK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    if let Some(file) = log_file {
        builder.with_writer(std::sync::Mutex::new(file)).init();
    } else {
        builder.with_writer(std::io::sink).init();
    }
}

/// Walk the process snapshot to find our parent's PID. Mirrors the
/// helper in `local-pipe/src/server.rs` — duplicated here so the hook
/// stays a standalone binary with no cross-crate cycle.
fn parent_pid() -> u32 {
    use std::mem::size_of;

    #[repr(C)]
    struct ProcessEntry32 {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut core::ffi::c_void;
        fn Process32FirstW(snapshot: *mut core::ffi::c_void, entry: *mut ProcessEntry32) -> i32;
        fn Process32NextW(snapshot: *mut core::ffi::c_void, entry: *mut ProcessEntry32) -> i32;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot as isize == -1 {
        return 0;
    }
    let mut entry: ProcessEntry32 = unsafe { std::mem::zeroed() };
    entry.dw_size = size_of::<ProcessEntry32>() as u32;
    let self_pid = std::process::id();
    let mut ppid: u32 = 0;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        if entry.th32_process_id == self_pid {
            ppid = entry.th32_parent_process_id;
            break;
        }
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    ppid
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p tg-hook`
Expected: clean compile.

- [ ] **Step 3: Lint**

Run: `cargo clippy -p tg-hook --all-targets -- -D warnings`
Expected: no warnings (pedantic).

- [ ] **Step 4: Format**

Run: `cargo fmt -p tg-hook`

- [ ] **Step 5: Commit**

```powershell
git add crates/tg-hook/src/main.rs
git commit -m "feat(tg-hook): wire main entrypoint with ctrl_c + JSON response"
```

---

## Milestone 7 — Documentation and integration

### Task 9: README with `settings.json` snippet

**Files:**
- Create: `crates/tg-hook/README.md`

- [ ] **Step 1: Write the README**

```markdown
# tg-hook — Claude Code Stop hook for TelegramMCP

Two-way Telegram control for a Claude Code session: when Claude finishes
a turn, `tg-hook` sends a wakeup message to a configured chat and blocks
the Stop hook for up to 60 minutes waiting for the user's reply. The
reply is fed back to Claude as a new turn; on timeout, Claude is asked
to send a brief status update and the cycle repeats.

## Build

```powershell
cargo build -p tg-hook --release
```

Binary lands at `target\release\tg-hook.exe`.

## Wire into Claude Code

Add a Stop hook in `.claude\settings.json` (project or user scope):

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "D:/Work/Programming/MCP/Telegram/target/release/tg-hook.exe --chat alerts --message \"Idle; what's next?\" --timeout-secs 3600",
            "timeout": 3700000
          }
        ]
      }
    ]
  }
}
```

* `--chat` accepts an alias defined in TelegramMCP's `[aliases]` table,
  or a numeric chat id.
* `timeout` (milliseconds, Claude Code's hook timeout) must be **larger
  than** `--timeout-secs * 1000` so Claude Code does not kill the hook
  before its own deadline.

## Behavior

| Event                          | Hook action                                                 | Claude reaction                  |
|--------------------------------|-------------------------------------------------------------|----------------------------------|
| Telegram reply arrives         | stdout `{"decision":"block","reason":"User replied: ..."}` | continues with the reply as input |
| 60 min elapses with no reply   | stdout `{"decision":"block","reason":"<retry-message>"}`   | continues (likely sends a "still here" message that re-fires this hook) |
| Ctrl+C in Claude Code          | hook exits 0 with empty stdout                              | Claude stops normally            |

While blocked, the hook prints `Waiting on keyboard interrupt or external message (60 min)` to stderr; Claude Code surfaces that line to the user.

## Logs

Per-PID at `%LOCALAPPDATA%\TelegramMCP\logs\tg-hook-<pid>.log`. Set `TG_HOOK_LOG=debug` for verbose output.

## Limitations

* **Windows only** (named pipes).
* **Requires a running TelegramMCP** spawned by the same Claude Code session — the hook locates it by `CLAUDE_SESSION_ID` or PPID via the discovery file.
* **No exponential backoff** on polling; 2 s by default. The pipe + SQLite read is cheap, but `--poll-secs` can be raised if many parallel sessions share the same Telegram bot.
```

- [ ] **Step 2: Commit**

```powershell
git add crates/tg-hook/README.md
git commit -m "docs(tg-hook): README with settings.json wiring"
```

### Task 10: Whole-workspace verification

- [ ] **Step 1: Build everything**

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 2: Run every test**

Run: `cargo test --workspace --all-targets -- --test-threads=1`
Expected: all green. (`--test-threads=1` for `tg-hook`'s pipe-server tests; other crates are tolerant of either.)

- [ ] **Step 3: Clippy + fmt**

Run: `cargo fmt --all -- --check`
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Manual smoke (optional, requires a real bot)**

  1. Start the server: `cargo run -p mcp-server -- --config config.toml`
  2. In a separate shell, simulate a Stop hook invocation:
     ```powershell
     '{"session_id":"manual","stop_hook_active":false}' | cargo run -p tg-hook -- --chat alerts --message "manual test" --timeout-secs 60
     ```
  3. Reply to the bot on Telegram within 60 s. The hook should print a `decision: block` JSON line and exit 0.
  4. Re-run and let it time out; observe the retry-message JSON.
  5. Re-run and Ctrl+C; observe an empty stdout exit.

- [ ] **Step 5: Final commit (if there are residual whitespace/format-only changes)**

```powershell
git status
# if dirty:
git add -A
git commit -m "chore: workspace verification — fmt/clippy clean"
```

---

## Self-review checklist

* Spec coverage — every locked-in decision (60-min timeout, retry-back-to-Claude semantics, Ctrl+C escape, stderr status line, MCP-via-pipe transport, discovery match) has at least one corresponding task. ✅
* Placeholder scan — every step contains the actual file path, the actual code (Rust or PowerShell), and the expected command output. No "TBD" / "implement later" anywhere.
* Type consistency — `HookArgs`, `HookError`, `PipeClient`, `McpClient`, `Outcome` names line up across Tasks 2/4/5/6/7/8. The `run()` signature in Task 7 matches its callsite in Task 8. `pick_inbound_reply` is private to `run.rs` and only used internally — no cross-task name drift.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-14-telegram-stop-hook.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
