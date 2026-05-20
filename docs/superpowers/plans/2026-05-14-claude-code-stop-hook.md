# Claude Code Stop Hook Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Windows-only Rust binary `tg-hook` to be wired into Claude Code's `Stop` hook in `settings.json`. When Claude finishes a turn, the hook sends a wakeup Telegram message via the running TelegramMCP server (over its local named pipe), blocks waiting for a user reply for up to 60 min, and returns the reply to Claude Code so the session can continue — turning Telegram into a remote control loop for an unattended Claude Code session.

**Architecture:** New capability crate `tg-hook` producing a single binary. The hook is an **MCP client** of the already-running TelegramMCP server — it does not embed `teloxide` or read the SQLite history directly. It locates the right MCP instance via the discovery files written by `local-pipe`, connects to the Windows named pipe, sends `AUTH <token>\n`, completes the MCP handshake, calls `tg_send_message` to wake the user, then polls `tg_history_messages` filtering for `direction == "in"` and `message_id > baseline` until either an inbound message arrives, 60 min elapses (→ retry-on-next-stop), or the user presses Ctrl+C (→ graceful exit so Claude stops normally). Live status is printed to stderr; the final block/no-block decision is printed as JSON on stdout per Claude Code's Stop-hook protocol.

**Tech Stack:** Rust (stable, edition 2024, Windows-only crate), `tokio` (named-pipe + signal::ctrl_c), `serde`/`serde_json` (MCP JSON-RPC + Stop-hook stdin), `thiserror`, `tracing` (logged to file, not stderr — stderr is reserved for user-visible status). No `rmcp` client dependency; we hand-roll the tiny subset of MCP we need (initialize, tools/call) since the call surface is two methods.

**Spec context:**
- Main project design: [../specs/2026-05-13-telegram-mcp-design.md](../specs/2026-05-13-telegram-mcp-design.md)
- Main project plan: [./2026-05-13-telegram-mcp.md](./2026-05-13-telegram-mcp.md)
- Existing crate that the hook depends on conceptually: [`crates/local-pipe/`](../../../crates/local-pipe/) — read its `src/lib.rs`, `src/discovery.rs`, `src/auth.rs` end-to-end before writing the hook's client side.

---

## What the hook talks to (read this before coding)

The TelegramMCP server already serves MCP JSON-RPC over **two** transports concurrently (see `crates/mcp-server/src/main.rs` near the end of `main()`):

1. **stdio** — for Claude Code itself.
2. **Windows named pipe** at `\\.\pipe\telegrammcp-<server-pid>` — for local hook processes.

Connecting to the pipe requires:
1. Find the right server PID via the discovery file. Discovery files live in `%LOCALAPPDATA%\TelegramMCP\discovery\<pid>.json`. Each contains `pid`, `ppid`, `pipe`, `token`, `session_id`, `started_at`. The TelegramMCP server's `ppid` is the Claude Code process that spawned it.
2. Open the named pipe (`ClientOptions::new().open(pipe_path)`).
3. Send the literal line `AUTH <token>\n` as the very first bytes.
4. After that, the pipe is a vanilla bidirectional byte stream speaking MCP JSON-RPC line-delimited.

Discovery match priority (most-specific first):
1. **`session_id`** — if the hook's `$env:CLAUDE_SESSION_ID` matches a discovery record's `session_id`, use it.
2. **`ppid`** — the hook's PPID is the Claude Code process; the server's `ppid` is also that same Claude Code process. Match on equal PPIDs.
3. If neither yields exactly one match, fail loudly. Never silently pick "the most recent one" — wrong server is worse than no server.

## How Claude Code's Stop hook protocol works (read this too)

Claude Code invokes the configured Stop hook command, passing a JSON document on the hook's stdin:

```json
{
  "session_id": "abc...",
  "transcript_path": "/path/to/transcript.jsonl",
  "cwd": "/abs/cwd",
  "hook_event_name": "Stop",
  "stop_hook_active": false
}
```

The hook controls Claude Code's next action via its **stdout** — a single JSON object printed on exit (after all stderr output):

- `{"decision": "block", "reason": "<text>"}` — Claude does NOT stop. The `reason` is fed back as if it were a new user message. Claude responds to it, then the cycle repeats (when Claude next stops, the hook fires again, this time with `stop_hook_active: true`).
- `{}` or any non-`block` JSON or no output — Claude stops normally.

Hook **stderr** is surfaced to the user in the Claude Code UI as status text while the hook runs. We use stderr for the "Waiting on keyboard interrupt or external message" status line.

Hook timeout in Claude Code is configured per-entry in `settings.json` as `timeout` (seconds). We set it to **3700** (just over 60 min) so the hook has time to do its own 60-min wait without being killed.

---

## File structure produced by this plan

```
TelegramMCP/
├── Cargo.toml                              (modified: add crate to workspace + deps)
├── crates/
│   └── tg-hook/                            (NEW crate)
│       ├── Cargo.toml
│       ├── README.md                       short user-facing usage
│       └── src/
│           ├── main.rs                     #[tokio::main], arg parsing, top-level flow
│           ├── stop_hook_input.rs          serde struct for stdin JSON
│           ├── discover.rs                 enumerate discovery files, pick the match
│           ├── pipe.rs                     NamedPipeClient connect + AUTH writeout
│           ├── mcp.rs                      tiny MCP client: initialize + tools/call
│           ├── flow.rs                     send_wakeup, poll_for_reply, run()
│           └── tests/                      (unit tests inline per file via #[cfg(test)])
├── docs/
│   └── claude-code-hook.md                 (NEW) settings.json wiring + troubleshooting
└── (no changes to mcp-server crate)
```

The hook does **not** touch `mcp-server`'s code — it only uses its named-pipe MCP interface. Zero changes to the running server are required for v1.

---

## Milestone 1 — Crate scaffold

### Task 1: Create the `tg-hook` crate skeleton

**Files:**
- Create: `crates/tg-hook/Cargo.toml`
- Create: `crates/tg-hook/src/main.rs`
- Modify: `Cargo.toml` (workspace root) — add `tg-hook = { path = "crates/tg-hook" }` to `[workspace.dependencies]` (so the path is interned even though only the binary consumes it).

- [ ] **Step 1: Add the crate as a workspace member**

The workspace already uses `members = ["crates/*"]` in the root `Cargo.toml`, so creating `crates/tg-hook/` is enough — no edit needed to `[workspace]`.

- [ ] **Step 2: Write `crates/tg-hook/Cargo.toml`**

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

[target.'cfg(windows)'.dependencies]
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = { workspace = true }
dirs = { workspace = true }

[target.'cfg(not(windows))'.dependencies]
# Hook is Windows-only. On non-Windows the crate compiles but the binary
# is a stub that exits non-zero — keeps `cargo check --workspace` green.

[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 3: Write a minimal placeholder `src/main.rs` so `cargo build` passes**

```rust
//! `tg-hook` — Claude Code Stop hook for TelegramMCP.
//!
//! Sends a Telegram wakeup, blocks waiting for a reply via the running
//! TelegramMCP server's local named pipe, and prints a Stop-hook decision
//! JSON to stdout. Windows-only. See `crates/tg-hook/README.md`.

#[cfg(not(windows))]
fn main() {
    eprintln!("tg-hook: Windows-only. This platform is not supported.");
    std::process::exit(2);
}

#[cfg(windows)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Wired up in subsequent tasks.
    Ok(())
}
```

- [ ] **Step 4: Build the workspace to confirm the crate slots in**

Run: `cargo check --workspace`
Expected: zero errors, zero warnings (the workspace already uses `-D warnings` in clippy, but `cargo check` is enough at this stage). If you see a warning about `tracing-subscriber` being an unused dep, leave it — Task 9 wires it in.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/tg-hook
git commit -m "feat(tg-hook): scaffold Windows-only Claude Code Stop hook crate"
```

---

### Task 2: Parse the Stop-hook stdin JSON document

**Files:**
- Create: `crates/tg-hook/src/stop_hook_input.rs`
- Modify: `crates/tg-hook/src/main.rs` (add `mod stop_hook_input;` only — wiring is later)

The Claude Code Stop hook sends a JSON object on the hook's stdin. We only use three fields; the rest are forward-compatible junk we ignore.

- [ ] **Step 1: Write the type**

```rust
//! Stdin payload Claude Code sends to Stop hooks.

use serde::Deserialize;

/// Subset of fields we use from the Claude Code Stop-hook stdin JSON.
///
/// The full payload has more fields (`cwd`, `transcript_path`, ...) but the
/// hook only needs `session_id` (for discovery matching) and
/// `stop_hook_active` (logged for visibility — we ignore it for routing).
#[derive(Debug, Deserialize)]
pub struct StopHookInput {
    /// Claude Code session id. Used as the most-specific discovery match key.
    /// Optional because older Claude Code builds may not emit it.
    pub session_id: Option<String>,
    /// Set when this is a re-fire because a previous hook invocation returned
    /// `decision: block`. We log it but do not change behaviour — the user's
    /// "retry forever until a reply or Ctrl+C" policy is explicit.
    #[serde(default)]
    pub stop_hook_active: bool,
}

impl StopHookInput {
    /// Parse from a reader. Returns a stub with all fields defaulted if the
    /// stdin is empty (rare — Claude Code always sends something, but local
    /// manual invocation `echo {} | tg-hook ...` should also work).
    pub fn from_reader<R: std::io::Read>(mut r: R) -> Result<Self, serde_json::Error> {
        let mut buf = String::new();
        // Ignore I/O errors here; an empty stdin (closed pipe) yields "" and
        // we treat that as an empty JSON object.
        let _ = std::io::Read::read_to_string(&mut r, &mut buf);
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            return Ok(Self {
                session_id: None,
                stop_hook_active: false,
            });
        }
        serde_json::from_str(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_payload() {
        let json = br#"{"session_id":"abc","stop_hook_active":true,"cwd":"/x"}"#;
        let v = StopHookInput::from_reader(&json[..]).unwrap();
        assert_eq!(v.session_id.as_deref(), Some("abc"));
        assert!(v.stop_hook_active);
    }

    #[test]
    fn parses_minimal_payload() {
        let json = b"{}";
        let v = StopHookInput::from_reader(&json[..]).unwrap();
        assert_eq!(v.session_id, None);
        assert!(!v.stop_hook_active);
    }

    #[test]
    fn parses_empty_stdin() {
        let v = StopHookInput::from_reader(&b""[..]).unwrap();
        assert_eq!(v.session_id, None);
        assert!(!v.stop_hook_active);
    }
}
```

- [ ] **Step 2: Declare the module in `main.rs`**

In `crates/tg-hook/src/main.rs`, inside the `#[cfg(windows)]` section, add:

```rust
#[cfg(windows)]
mod stop_hook_input;
```

(If you organise the cfg differently, e.g. via a `#[cfg(windows)] mod windows { ... }`, declare it inside that wrapper. Either is fine; match whatever you settled on in Task 1.)

- [ ] **Step 3: Run the unit tests**

Run: `cargo test -p tg-hook --lib`
Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/tg-hook
git commit -m "feat(tg-hook): parse Claude Code Stop-hook stdin JSON"
```

---

## Milestone 2 — Find and connect to the right MCP server

### Task 3: Read discovery files and pick the right MCP server

**Files:**
- Create: `crates/tg-hook/src/discover.rs`
- Modify: `crates/tg-hook/src/main.rs` (declare module)

Discovery files are written by `crates/local-pipe/src/discovery.rs`. We re-decode them here ourselves rather than depending on `local-pipe` from the hook crate — the hook is a separate component that talks to the server **over the wire**, not via shared types. (Keeps the dep graph honest: changing internal `DiscoveryRecord` fields shouldn't recompile the hook unless they're wire-visible.)

- [ ] **Step 1: Write the discovery selector**

```rust
//! Locate the right TelegramMCP instance among discovery files.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Subset of the `DiscoveryRecord` JSON we need. Mirrors
/// `local-pipe::DiscoveryRecord` but is locally redeclared so the hook crate
/// stays decoupled from `local-pipe`'s in-process types.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveryFile {
    /// Server PID — only used for log lines.
    pub pid: u32,
    /// Parent PID of the server (= Claude Code's PID).
    pub ppid: u32,
    /// Full named-pipe path (e.g. `\\.\pipe\telegrammcp-12345`).
    pub pipe: String,
    /// Per-instance auth token (write `AUTH <token>\n` as first bytes).
    pub token: String,
    /// Claude Code session id when available. Highest-priority match key.
    pub session_id: Option<String>,
}

/// Errors from discovery selection.
#[derive(Debug, Error)]
pub enum DiscoverError {
    /// Couldn't locate `%LOCALAPPDATA%`.
    #[error("could not resolve %LOCALAPPDATA% to look for discovery files")]
    NoLocalAppData,
    /// I/O error while listing or reading discovery files.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// No discovery file matched. The hook can't proceed.
    #[error("no TelegramMCP discovery file matched (session_id={session_id:?}, ppid={ppid})")]
    NoMatch {
        /// session_id the hook tried to match.
        session_id: Option<String>,
        /// PPID the hook tried to match.
        ppid: u32,
    },
    /// More than one file matched after applying all match keys. We refuse
    /// to guess — the user should kill stale servers or set $env:CLAUDE_SESSION_ID.
    #[error("{n} discovery files matched; cannot disambiguate")]
    Ambiguous {
        /// Number of files matched.
        n: usize,
    },
}

/// Resolve `%LOCALAPPDATA%\TelegramMCP\discovery`.
pub fn discovery_dir() -> Result<PathBuf, DiscoverError> {
    let base = dirs::data_local_dir().ok_or(DiscoverError::NoLocalAppData)?;
    Ok(base.join("TelegramMCP").join("discovery"))
}

/// Enumerate all parseable discovery files in `dir`. Unparseable / unreadable
/// files are skipped silently — a stale file from a crashed previous server
/// shouldn't break a healthy current one.
pub fn list_files(dir: &Path) -> Result<Vec<DiscoveryFile>, DiscoverError> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rec) = serde_json::from_str::<DiscoveryFile>(&raw) else {
            continue;
        };
        out.push(rec);
    }
    Ok(out)
}

/// Pick the right discovery record using session_id (preferred) then ppid.
///
/// Returns `NoMatch` if no candidate matched, `Ambiguous` if more than one
/// did even after applying the most-specific available key.
pub fn pick(
    candidates: &[DiscoveryFile],
    session_id: Option<&str>,
    hook_ppid: u32,
) -> Result<DiscoveryFile, DiscoverError> {
    // 1. Try session_id match (only when the hook has one).
    if let Some(sid) = session_id {
        let hits: Vec<_> = candidates
            .iter()
            .filter(|c| c.session_id.as_deref() == Some(sid))
            .collect();
        match hits.len() {
            0 => {} // fall through to PPID
            1 => return Ok(hits[0].clone()),
            n => return Err(DiscoverError::Ambiguous { n }),
        }
    }
    // 2. Try PPID match.
    let hits: Vec<_> = candidates.iter().filter(|c| c.ppid == hook_ppid).collect();
    match hits.len() {
        0 => Err(DiscoverError::NoMatch {
            session_id: session_id.map(str::to_string),
            ppid: hook_ppid,
        }),
        1 => Ok(hits[0].clone()),
        n => Err(DiscoverError::Ambiguous { n }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pid: u32, ppid: u32, sid: Option<&str>) -> DiscoveryFile {
        DiscoveryFile {
            pid,
            ppid,
            pipe: format!(r"\\.\pipe\telegrammcp-{pid}"),
            token: "tok".into(),
            session_id: sid.map(str::to_string),
        }
    }

    #[test]
    fn picks_by_session_id() {
        let cs = vec![rec(1, 100, Some("A")), rec(2, 100, Some("B"))];
        let got = pick(&cs, Some("B"), 100).unwrap();
        assert_eq!(got.pid, 2);
    }

    #[test]
    fn falls_back_to_ppid_when_session_misses() {
        let cs = vec![rec(1, 100, None), rec(2, 200, None)];
        let got = pick(&cs, Some("nope"), 200).unwrap();
        assert_eq!(got.pid, 2);
    }

    #[test]
    fn ambiguous_ppid_is_an_error() {
        let cs = vec![rec(1, 100, None), rec(2, 100, None)];
        let err = pick(&cs, None, 100).unwrap_err();
        assert!(matches!(err, DiscoverError::Ambiguous { n: 2 }));
    }

    #[test]
    fn no_match_at_all_is_an_error() {
        let cs = vec![rec(1, 100, None)];
        let err = pick(&cs, None, 999).unwrap_err();
        assert!(matches!(err, DiscoverError::NoMatch { .. }));
    }
}
```

- [ ] **Step 2: Declare the module in `main.rs`**

```rust
#[cfg(windows)]
mod discover;
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test -p tg-hook --lib`
Expected: 4 new tests pass, plus the 3 from Task 2 = 7 total.

- [ ] **Step 4: Commit**

```bash
git add crates/tg-hook
git commit -m "feat(tg-hook): discovery file enumeration + match by session_id/ppid"
```

---

### Task 4: Look up the hook's own PPID (Windows-only)

**Files:**
- Modify: `crates/tg-hook/src/discover.rs` — add `parent_pid()` helper.

We could vendor the same `parent_pid()` from `crates/local-pipe/src/server.rs`, but it's small enough to redeclare here so the hook doesn't grow an FFI surface area it doesn't otherwise need. Copy verbatim from the existing impl (audit it — that's the only file using `Toolhelp32`).

- [ ] **Step 1: Read the existing impl**

Open `crates/local-pipe/src/server.rs` lines around `fn parent_pid()` and `struct ProcessEntry32`. That's the canonical impl in this repo. Read it end-to-end before copying — the `#[link(name = "kernel32")]` block, the `TH32CS_SNAPPROCESS = 0x0000_0002` constant, and the lint allowances all transfer over.

- [ ] **Step 2: Append `parent_pid()` to `discover.rs`**

At the bottom of `crates/tg-hook/src/discover.rs`, append:

```rust
/// Look up our own parent process id via the Win32 process snapshot.
///
/// Best-effort: returns `0` if the snapshot or walk fails. Callers should
/// treat a zero return as "no PPID known" and surface a clear error rather
/// than blindly matching `ppid == 0` discovery files.
#[allow(
    clippy::cast_possible_truncation,
    reason = "Win32 ProcessEntry32 fields are fixed-width u32"
)]
pub fn parent_pid() -> u32 {
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

- [ ] **Step 3: Sanity-check with a runtime probe (not a permanent test)**

A unit test that asserts a specific PPID would be flaky. Instead, verify the call doesn't panic via a smoke-style check.

Add to the existing `#[cfg(test)] mod tests` block in `discover.rs`:

```rust
    #[test]
    fn parent_pid_returns_nonzero_under_cargo_test() {
        // Under `cargo test`, the test binary is a child of cargo. Any
        // sane Windows machine returns a positive parent PID. We don't
        // assert which PID — just that the FFI call works.
        let ppid = super::parent_pid();
        assert!(ppid > 0, "expected a parent process; got 0");
    }
```

Run: `cargo test -p tg-hook --lib`
Expected: 8 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/tg-hook
git commit -m "feat(tg-hook): Win32 parent_pid helper for discovery match"
```

---

### Task 5: Connect to the pipe and send AUTH

**Files:**
- Create: `crates/tg-hook/src/pipe.rs`
- Modify: `crates/tg-hook/src/main.rs` (declare module)

- [ ] **Step 1: Write the pipe client**

```rust
//! Windows named-pipe client: open the pipe, send `AUTH <token>\n`,
//! hand back the connected pipe ready for MCP JSON-RPC.

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

/// Errors from connecting and authenticating to the pipe.
#[derive(Debug, Error)]
pub enum PipeClientError {
    /// I/O error opening the pipe (server not listening, ACL deny, ...).
    #[error("opening pipe {path}: {source}")]
    Open {
        /// The pipe path we tried.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// I/O error writing the AUTH line.
    #[error("writing AUTH line: {0}")]
    Write(std::io::Error),
}

/// Open `path` and send `AUTH <token>\n`. Returns the connected pipe with
/// the auth line fully flushed; the next bytes either side writes are MCP
/// JSON-RPC.
///
/// Does not retry on `ERROR_PIPE_BUSY` (`231`) — the server's accept loop
/// builds a fresh pipe instance per connection (`ServerOptions::new().create(path)`)
/// so a busy condition would indicate the server is wedged.
pub async fn connect_and_auth(
    path: &str,
    token: &str,
) -> Result<NamedPipeClient, PipeClientError> {
    let mut pipe = ClientOptions::new()
        .open(path)
        .map_err(|source| PipeClientError::Open {
            path: path.to_string(),
            source,
        })?;
    let auth = format!("AUTH {token}\n");
    pipe.write_all(auth.as_bytes())
        .await
        .map_err(PipeClientError::Write)?;
    pipe.flush().await.map_err(PipeClientError::Write)?;
    Ok(pipe)
}
```

- [ ] **Step 2: Declare the module**

```rust
#[cfg(windows)]
mod pipe;
```

- [ ] **Step 3: Build and clippy**

Run: `cargo clippy -p tg-hook --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/tg-hook
git commit -m "feat(tg-hook): named-pipe client with AUTH handshake"
```

---

## Milestone 3 — MCP client + send/poll flow

### Task 6: Hand-rolled MCP client (initialize + tools/call)

**Files:**
- Create: `crates/tg-hook/src/mcp.rs`
- Modify: `crates/tg-hook/src/main.rs` (declare module)

We don't pull in `rmcp` as a client — its 0.2 client surface is heavier than the two RPCs we need. JSON-RPC over a line-delimited byte stream is small enough to inline.

- [ ] **Step 1: Write the client**

```rust
//! Minimal MCP JSON-RPC client speaking line-delimited JSON over an
//! `AsyncRead + AsyncWrite` (the pipe from `pipe::connect_and_auth`).
//!
//! Only two methods are needed: `initialize` (+ the `notifications/initialized`
//! follow-up) and `tools/call`.

use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Errors from MCP request/response handling.
#[derive(Debug, Error)]
pub enum McpError {
    /// I/O error reading or writing the underlying transport.
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    /// JSON parse error on an inbound response.
    #[error("malformed response json: {0}")]
    Decode(#[from] serde_json::Error),
    /// Server returned a JSON-RPC error object.
    #[error("server error {code}: {message}")]
    Rpc {
        /// JSON-RPC error code.
        code: i64,
        /// JSON-RPC error message.
        message: String,
    },
    /// Connection closed before a response arrived.
    #[error("connection closed before response")]
    Eof,
}

/// MCP client over an async bidirectional byte stream.
pub struct McpClient<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> {
    reader: BufReader<tokio::io::ReadHalf<S>>,
    writer: tokio::io::WriteHalf<S>,
    next_id: u64,
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> McpClient<S> {
    /// Wrap an already-connected transport. Caller is responsible for any
    /// out-of-band handshake (e.g. our pipe's `AUTH <token>\n`) before this.
    pub fn new(stream: S) -> Self {
        let (r, w) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(r),
            writer: w,
            next_id: 1,
        }
    }

    /// Drive the MCP `initialize` request + `notifications/initialized`.
    /// Returns the `result` object from the server's initialize response.
    pub async fn initialize(&mut self) -> Result<Value, McpError> {
        let result = self
            .send_request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "tg-hook", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;
        // Required by the MCP spec — server may refuse calls until it arrives.
        self.send_notification("notifications/initialized", json!({})).await?;
        Ok(result)
    }

    /// Call a tool by name with the given arguments. Returns the `result`
    /// envelope (typically `{ "content": [ { "type": "text", "text": "..." } ] }`).
    pub async fn call_tool(&mut self, name: &str, args: Value) -> Result<Value, McpError> {
        self.send_request(
            "tools/call",
            json!({ "name": name, "arguments": args }),
        )
        .await
    }

    async fn send_request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_line(&req).await?;
        loop {
            let line = self.read_line().await?;
            let v: Value = serde_json::from_str(&line)?;
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                // Notifications and out-of-order responses are skipped.
                continue;
            }
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("(no message)")
                    .to_string();
                return Err(McpError::Rpc { code, message });
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn send_notification(&mut self, method: &str, params: Value) -> Result<(), McpError> {
        let n = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_line(&n).await
    }

    async fn write_line(&mut self, v: &Value) -> Result<(), McpError> {
        let mut s = serde_json::to_string(v)?;
        s.push('\n');
        self.writer.write_all(s.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn read_line(&mut self) -> Result<String, McpError> {
        let mut buf = String::new();
        let n = self.reader.read_line(&mut buf).await?;
        if n == 0 {
            return Err(McpError::Eof);
        }
        Ok(buf)
    }
}

/// Decoded `result` of `tg_send_message`. The tool returns its payload as a
/// JSON string inside a `text` content block; we peel that off here.
#[derive(Debug, Deserialize)]
pub struct SendMessageResult {
    /// Numeric chat id the message landed in.
    pub chat_id: i64,
    /// Telegram `message_id` of the sent message.
    pub message_id: i64,
    /// Unix timestamp of the send.
    #[serde(default)]
    #[allow(dead_code, reason = "kept for diagnostic logging")]
    pub date: i64,
}

/// Extract the JSON-string payload from an MCP `tools/call` result envelope.
///
/// The TelegramMCP tools emit `content: [ { type: "text", text: "<json>" } ]`.
/// This helper takes that result and returns the parsed inner JSON.
pub fn unwrap_text_payload<T: serde::de::DeserializeOwned>(result: &Value) -> Result<T, McpError> {
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Rpc {
            code: -32603,
            message: "tool result missing content[0].text".into(),
        })?;
    serde_json::from_str::<T>(text).map_err(McpError::Decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_send_message_payload() {
        let env = json!({
            "content": [
                { "type": "text", "text": "{\"chat_id\":42,\"message_id\":7,\"date\":1700000000}" }
            ]
        });
        let r: SendMessageResult = unwrap_text_payload(&env).unwrap();
        assert_eq!(r.chat_id, 42);
        assert_eq!(r.message_id, 7);
    }

    #[test]
    fn missing_content_is_an_error() {
        let env = json!({});
        let err = unwrap_text_payload::<SendMessageResult>(&env).unwrap_err();
        assert!(matches!(err, McpError::Rpc { .. }));
    }
}
```

- [ ] **Step 2: Declare the module**

In `main.rs` add:

```rust
#[cfg(windows)]
mod mcp;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p tg-hook --lib`
Expected: 10 tests pass (2 new).

- [ ] **Step 4: Commit**

```bash
git add crates/tg-hook
git commit -m "feat(tg-hook): minimal MCP client (initialize + tools/call)"
```

---

### Task 7: Send wakeup + poll for reply

**Files:**
- Create: `crates/tg-hook/src/flow.rs`
- Modify: `crates/tg-hook/src/main.rs` (declare module)

This is the core loop. It accepts a connected `McpClient`, sends `tg_send_message`, then polls `tg_history_messages` with `after_message_id = sent_message_id` filtering for `direction == "in"`. Returns one of:
- `Outcome::Reply { text }` — an inbound message arrived; pass its text back to Claude.
- `Outcome::Timeout` — the 60-min window elapsed; hook will return the retry-reason.
- `Outcome::Interrupted` — the caller's `cancel` future fired; hook will exit cleanly without blocking.

- [ ] **Step 1: Write the flow module**

```rust
//! High-level send-and-wait orchestration. Pure function over an
//! `McpClient` — does no I/O of its own except via that client.

use crate::mcp::{unwrap_text_payload, McpClient, McpError, SendMessageResult};
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::time::{Duration, Instant};

/// What happened during the send-then-wait flow.
#[derive(Debug)]
pub enum Outcome {
    /// User replied. `text` is the inbound message body (may be empty for
    /// media-only messages — caller decides how to handle that).
    Reply {
        /// The inbound message text, lossily converted (empty when absent).
        text: String,
    },
    /// `wait_for` elapsed without a reply. The caller will emit a retry
    /// reason so Claude continues and the next Stop fires the hook again.
    Timeout,
    /// The cancel future fired (e.g. Ctrl+C). Caller will exit cleanly with
    /// no `decision: block`, so Claude stops normally.
    Interrupted,
}

/// One row from `tg_history_messages`. Only the fields we need.
#[derive(Debug, Deserialize)]
struct HistoryRow {
    message_id: i64,
    direction: String,
    #[serde(default)]
    text: Option<String>,
}

/// Send the wakeup, then poll history until reply / timeout / cancel.
///
/// `chat` is passed through to `tg_send_message` unchanged — accepts a
/// numeric id-as-string or an alias.
///
/// `cancel` is awaited concurrently with the polling sleep. Pass
/// `std::future::pending()` to disable cancellation.
pub async fn send_and_wait<S, F>(
    client: &mut McpClient<S>,
    chat: &str,
    wakeup_text: &str,
    wait_for: Duration,
    poll_every: Duration,
    cancel: F,
) -> Result<Outcome, McpError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: Future<Output = ()>,
{
    // 1. Send the wakeup. Record (chat_id, message_id) — chat_id is needed
    //    for polling; message_id becomes the after_message_id baseline.
    let send_envelope = client
        .call_tool(
            "tg_send_message",
            json!({ "chat": chat, "text": wakeup_text }),
        )
        .await?;
    let sent: SendMessageResult = unwrap_text_payload(&send_envelope)?;
    let baseline = sent.message_id;
    let chat_id = sent.chat_id;

    // 2. Poll loop. `cancel` is pinned once and `&mut cancel` is re-borrowed
    //    each iteration so a not-yet-ready Ctrl+C carries forward across
    //    loop turns. We `select!` on both the sleep AND the history call so
    //    a Ctrl+C during the network round-trip aborts immediately.
    let deadline = Instant::now() + wait_for;
    tokio::pin!(cancel);

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(Outcome::Timeout);
        }
        let sleep_for = poll_every.min(remaining);

        // Wait either for the poll tick or for cancel.
        tokio::select! {
            biased;
            () = &mut cancel => return Ok(Outcome::Interrupted),
            () = tokio::time::sleep(sleep_for) => {}
        }

        // Issue one history query, racing it against cancel so Ctrl+C
        // during the request also aborts cleanly.
        let history_call = client.call_tool(
            "tg_history_messages",
            json!({
                "chat": chat_id.to_string(),
                "after_message_id": baseline,
                "limit": 50,
            }),
        );
        tokio::pin!(history_call);
        let env = tokio::select! {
            biased;
            () = &mut cancel => return Ok(Outcome::Interrupted),
            r = &mut history_call => r?,
        };

        let rows: Vec<HistoryRow> = unwrap_text_payload(&env)?;

        // tg_history_messages returns newest-first. Take the OLDEST inbound
        // newer than baseline — that's the user's first reply.
        if let Some(first_reply) = rows
            .iter()
            .filter(|r| r.direction == "in" && r.message_id > baseline)
            .min_by_key(|r| r.message_id)
        {
            let text = first_reply.text.clone().unwrap_or_default();
            return Ok(Outcome::Reply { text });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_debug_does_not_panic() {
        // Sanity: the enum compiles + Debug works (smoke). The real flow
        // is exercised by tests/integration.rs in Task 10.
        let _ = format!("{:?}", Outcome::Timeout);
        let _ = format!("{:?}", Outcome::Reply { text: "x".into() });
        let _ = format!("{:?}", Outcome::Interrupted);
    }
}
```

Note on `Waker::noop()`: stabilised in Rust 1.85 (the workspace's MSRV per `rust-toolchain.toml`). If you're on an older toolchain locally, run `cargo +stable build`.

- [ ] **Step 2: Declare the module**

```rust
#[cfg(windows)]
mod flow;
```

- [ ] **Step 3: Build & test**

Run: `cargo test -p tg-hook --lib`
Expected: 11 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/tg-hook
git commit -m "feat(tg-hook): send-and-wait flow with polling + cancel"
```

---

## Milestone 4 — CLI + lifecycle

### Task 8: CLI arg parsing

**Files:**
- Create: `crates/tg-hook/src/args.rs`
- Modify: `crates/tg-hook/src/main.rs` (declare module)

Match `mcp-server/src/main.rs`'s manual arg-parser style — keeps clap out of the dep graph.

- [ ] **Step 1: Write the parser**

```rust
//! CLI argument parsing for `tg-hook`. Manual to match the style used
//! by `crates/mcp-server/src/main.rs` and keep `clap` out of the deps.

use std::time::Duration;
use thiserror::Error;

/// Parsed CLI args.
#[derive(Debug)]
pub struct Args {
    /// Telegram chat to send the wakeup to. Numeric id or alias defined
    /// in the server's `[aliases]` table — passed through verbatim to
    /// `tg_send_message`'s `chat` parameter.
    pub chat: String,
    /// Wakeup message body sent on every hook invocation.
    pub message: String,
    /// Retry reason returned to Claude when the wait elapses without a
    /// reply. Claude responds to this, then stops; the hook then fires
    /// again. The actual retry is at the Claude-conversation level, not
    /// inside the hook process.
    pub retry_message: String,
    /// Maximum time to wait for a Telegram reply per hook invocation.
    pub timeout: Duration,
    /// How often to poll `tg_history_messages` during the wait.
    pub poll_interval: Duration,
}

/// Errors from parsing CLI args.
#[derive(Debug, Error)]
pub enum ArgsError {
    /// A required flag was not supplied.
    #[error("missing required --{0} <value>")]
    Missing(&'static str),
    /// A flag was supplied but its value couldn't be parsed.
    #[error("--{flag}: {detail}")]
    Bad {
        /// The flag name.
        flag: &'static str,
        /// Detail of the parse failure.
        detail: String,
    },
    /// An unknown flag was supplied.
    #[error("unknown argument: {0}")]
    Unknown(String),
}

/// Parse `std::env::args()` (skipping argv[0]).
pub fn parse() -> Result<Args, ArgsError> {
    parse_from(std::env::args().skip(1))
}

/// Test-friendly: parse from any iterator of strings.
pub fn parse_from<I: IntoIterator<Item = String>>(it: I) -> Result<Args, ArgsError> {
    let mut chat = None;
    let mut message = None;
    let mut retry_message = None;
    let mut timeout_secs: Option<u64> = None;
    let mut poll_secs: Option<u64> = None;

    let mut it = it.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--chat" => chat = Some(require_value(&mut it, "chat")?),
            "--message" => message = Some(require_value(&mut it, "message")?),
            "--retry-message" => retry_message = Some(require_value(&mut it, "retry-message")?),
            "--timeout-secs" => {
                timeout_secs = Some(parse_secs(&mut it, "timeout-secs")?);
            }
            "--poll-secs" => {
                poll_secs = Some(parse_secs(&mut it, "poll-secs")?);
            }
            "--help" | "-h" => {
                eprintln!(include_str!("help.txt"));
                std::process::exit(0);
            }
            other => return Err(ArgsError::Unknown(other.to_string())),
        }
    }

    Ok(Args {
        chat: chat.ok_or(ArgsError::Missing("chat"))?,
        message: message.unwrap_or_else(|| "Claude finished a turn. What's next?".into()),
        retry_message: retry_message
            .unwrap_or_else(|| "No reply within the wait window — checking again.".into()),
        timeout: Duration::from_secs(timeout_secs.unwrap_or(3600)),
        poll_interval: Duration::from_secs(poll_secs.unwrap_or(5)),
    })
}

fn require_value<I: Iterator<Item = String>>(
    it: &mut I,
    flag: &'static str,
) -> Result<String, ArgsError> {
    it.next().ok_or(ArgsError::Missing(flag))
}

fn parse_secs<I: Iterator<Item = String>>(
    it: &mut I,
    flag: &'static str,
) -> Result<u64, ArgsError> {
    let raw = require_value(it, flag)?;
    raw.parse::<u64>().map_err(|e| ArgsError::Bad {
        flag,
        detail: format!("not a u64 ({e})"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(vs: &[&str]) -> Result<Args, ArgsError> {
        parse_from(vs.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_minimal() {
        let a = args(&["--chat", "alerts"]).unwrap();
        assert_eq!(a.chat, "alerts");
        assert_eq!(a.timeout, Duration::from_secs(3600));
        assert_eq!(a.poll_interval, Duration::from_secs(5));
    }

    #[test]
    fn parses_full() {
        let a = args(&[
            "--chat", "-1001234567890",
            "--message", "ping",
            "--retry-message", "still here",
            "--timeout-secs", "1800",
            "--poll-secs", "10",
        ])
        .unwrap();
        assert_eq!(a.chat, "-1001234567890");
        assert_eq!(a.message, "ping");
        assert_eq!(a.retry_message, "still here");
        assert_eq!(a.timeout, Duration::from_secs(1800));
        assert_eq!(a.poll_interval, Duration::from_secs(10));
    }

    #[test]
    fn missing_chat_is_an_error() {
        let err = args(&[]).unwrap_err();
        assert!(matches!(err, ArgsError::Missing("chat")));
    }

    #[test]
    fn unknown_flag_is_an_error() {
        let err = args(&["--chat", "x", "--what"]).unwrap_err();
        assert!(matches!(err, ArgsError::Unknown(_)));
    }

    #[test]
    fn non_numeric_timeout_is_an_error() {
        let err = args(&["--chat", "x", "--timeout-secs", "soon"]).unwrap_err();
        assert!(matches!(err, ArgsError::Bad { flag: "timeout-secs", .. }));
    }
}
```

- [ ] **Step 2: Write `crates/tg-hook/src/help.txt`**

```text
tg-hook — Claude Code Stop hook for TelegramMCP.

USAGE:
  tg-hook --chat <id_or_alias>
          [--message <text>]
          [--retry-message <text>]
          [--timeout-secs <int>]   default 3600
          [--poll-secs <int>]      default 5

The hook reads Claude Code's Stop-hook JSON payload from stdin, locates the
running TelegramMCP server via its discovery file (matched on session_id
then PPID), and connects to its local named pipe. It then sends the wakeup
message to <chat>, polls history until either a reply arrives or
--timeout-secs elapses, and prints the appropriate Stop-hook decision
JSON to stdout.

Reply  -> { "decision": "block", "reason": "<reply text>" }
Timeout-> { "decision": "block", "reason": "<retry message>" }
Ctrl+C -> exit 0 with no JSON, so Claude Code stops normally.

Live status while blocked is written to stderr.

ENV:
  CLAUDE_SESSION_ID   forwarded by Claude Code; used as the primary
                      discovery match key.
  TG_HOOK_LOG         tracing-subscriber filter (default: info). Logs
                      go to %LOCALAPPDATA%\TelegramMCP\logs\tg-hook-*.log,
                      never to stderr.
```

- [ ] **Step 3: Declare module**

```rust
#[cfg(windows)]
mod args;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p tg-hook --lib`
Expected: 16 tests pass (5 new).

- [ ] **Step 5: Commit**

```bash
git add crates/tg-hook
git commit -m "feat(tg-hook): CLI argument parsing + --help"
```

---

### Task 9: Wire main: stdin → discover → connect → run flow → emit decision

**Files:**
- Modify: `crates/tg-hook/src/main.rs` — replace placeholder body with the real orchestration.

- [ ] **Step 1: Rewrite `main.rs`**

Replace the body of the Windows main with this. Keep the non-Windows stub unchanged.

```rust
//! `tg-hook` — Claude Code Stop hook for TelegramMCP.
//!
//! Reads a Stop-hook JSON payload from stdin, finds the running TelegramMCP
//! server via the local-pipe discovery files, connects to its named pipe,
//! sends a wakeup Telegram message, blocks waiting for a reply (up to a
//! configurable timeout), and prints the resulting Stop-hook decision JSON
//! to stdout. Windows-only.

#[cfg(not(windows))]
fn main() {
    eprintln!("tg-hook: Windows-only. This platform is not supported.");
    std::process::exit(2);
}

#[cfg(windows)]
mod args;
#[cfg(windows)]
mod discover;
#[cfg(windows)]
mod flow;
#[cfg(windows)]
mod mcp;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod stop_hook_input;

#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let args = match args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("tg-hook: {e}");
            std::process::exit(2);
        }
    };

    let stdin_input =
        stop_hook_input::StopHookInput::from_reader(std::io::stdin()).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to parse Stop hook stdin; treating as empty");
            stop_hook_input::StopHookInput {
                session_id: None,
                stop_hook_active: false,
            }
        });
    tracing::info!(
        session_id = ?stdin_input.session_id,
        stop_hook_active = stdin_input.stop_hook_active,
        "tg-hook starting"
    );

    // Find the right MCP server.
    let dir = discover::discovery_dir()?;
    let candidates = discover::list_files(&dir)?;
    let ppid = discover::parent_pid();
    let record = discover::pick(&candidates, stdin_input.session_id.as_deref(), ppid)?;
    tracing::info!(
        pid = record.pid,
        pipe = %record.pipe,
        "matched discovery record"
    );

    // Open pipe + AUTH.
    let pipe = pipe::connect_and_auth(&record.pipe, &record.token).await?;
    let mut client = mcp::McpClient::new(pipe);
    client.initialize().await?;

    // Show live status the user can see in the Claude Code UI.
    eprintln!("Waiting on keyboard interrupt or external message");

    // Run send-and-wait with Ctrl+C as the cancel.
    let cancel = async {
        // ctrl_c() resolves the first time Ctrl+C is observed. If the host
        // never delivers one (e.g. no console attached), it just stays
        // pending forever — which is fine; the timeout still runs.
        let _ = tokio::signal::ctrl_c().await;
    };

    let outcome = flow::send_and_wait(
        &mut client,
        &args.chat,
        &args.message,
        args.timeout,
        args.poll_interval,
        cancel,
    )
    .await?;

    // Emit the Stop-hook decision JSON on stdout.
    match outcome {
        flow::Outcome::Reply { text } => {
            print_decision_block(&format_user_reply(&text));
        }
        flow::Outcome::Timeout => {
            print_decision_block(&args.retry_message);
        }
        flow::Outcome::Interrupted => {
            // Print nothing → Claude Code treats it as no decision → stop normally.
            tracing::info!("interrupted by user; Claude Code will stop normally");
        }
    }

    Ok(())
}

#[cfg(windows)]
fn print_decision_block(reason: &str) {
    // Single line of JSON on stdout. We use serde_json to escape the
    // reason properly (newlines, quotes) rather than format!() it.
    let v = serde_json::json!({
        "decision": "block",
        "reason": reason,
    });
    println!("{v}");
}

#[cfg(windows)]
fn format_user_reply(text: &str) -> String {
    // Prefix so the reason is unambiguous to Claude: it came from the user
    // over Telegram, not from the hook authoring its own instructions.
    if text.is_empty() {
        "User replied on Telegram with a non-text message (no text body).".to_string()
    } else {
        format!("User replied on Telegram: {text}")
    }
}

#[cfg(windows)]
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // Log to a per-PID file under %LOCALAPPDATA%\TelegramMCP\logs\. Never
    // touch stderr — Claude Code surfaces hook stderr to the user, and the
    // hook's stderr is reserved for the "Waiting on..." status line.
    let log_file = dirs::data_local_dir().and_then(|base| {
        let dir = base.join("TelegramMCP").join("logs");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("tg-hook-{}.log", std::process::id()));
        std::fs::File::create(path).ok()
    });
    let filter = EnvFilter::try_from_env("TG_HOOK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false);
    if let Some(file) = log_file {
        builder.with_writer(std::sync::Mutex::new(file)).init();
    } else {
        builder.with_writer(std::io::sink).init();
    }
}
```

- [ ] **Step 2: Build + clippy**

Run: `cargo clippy -p tg-hook --all-targets -- -D warnings`
Expected: zero warnings. If clippy complains about `clippy::similar_names` (e.g. `args`/`a`) or `clippy::single_match_else`, add a one-line `// reason` allow on the specific line.

- [ ] **Step 3: Smoke-test the help text**

Run: `cargo run -p tg-hook -- --help`
Expected: prints the help text from `help.txt`, exits 0.

- [ ] **Step 4: Smoke-test the missing-arg path**

Run: `cargo run -p tg-hook -- --chat`
Expected: `tg-hook: missing required --chat <value>`, exit 2.

- [ ] **Step 5: Smoke-test the no-server path**

With **no** TelegramMCP server running (kill any running instance first; check with `Get-Process TelegramMCP`):
Run: `echo '{}' | cargo run -p tg-hook -- --chat alerts`
Expected: prints a "no TelegramMCP discovery file matched" error and exits non-zero. (Exits via `anyhow::Result` → 1.)

- [ ] **Step 6: Commit**

```bash
git add crates/tg-hook
git commit -m "feat(tg-hook): wire main — stdin, discovery, MCP, flow, decision JSON"
```

---

## Milestone 5 — Integration tests against a real server

### Task 10: End-to-end test: hook drives a live TelegramMCP

**Files:**
- Create: `crates/tg-hook/tests/e2e.rs`
- Create: `crates/tg-hook/tests/common/mod.rs` — small spawn helper, modeled on `crates/mcp-server/tests/common/mod.rs`.

This is the canonical "does it actually work" test. We spawn a real `TelegramMCP` binary pointed at a `wiremock` fake Bot API and a tempdir history db, **then** spawn `tg-hook` and pipe Stop-hook JSON to its stdin. The hook should discover that server (single discovery file in the dir → unambiguous), send the message (fake Bot API records it), and then we simulate an inbound message by directly inserting into the history db so the hook's next poll finds it.

Direct DB writes from the test are a deliberate corner-cut — exercising the real long-poll updater here is out of scope. The hook code path being tested (poll → find inbound → emit decision JSON) is identical regardless of *how* the row appeared.

- [ ] **Step 1: Write `tests/common/mod.rs`**

```rust
//! Shared helpers for tg-hook end-to-end tests.

#![allow(
    dead_code,
    reason = "helpers; some unused per test"
)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests panic on infra failures"
)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// Path to the built `TelegramMCP` binary. Cargo populates
/// `CARGO_BIN_EXE_<name>` only for direct deps of this test target, so
/// `tg-hook`'s tests can't use `env!()` for it; we resolve at runtime
/// via the cargo metadata format.
pub fn telegram_mcp_binary() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at this crate's manifest.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // ../.. = workspace root; target/debug/TelegramMCP.exe (or no ext on non-win).
    let target = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("target")
        .join("debug")
        .join(if cfg!(windows) {
            "TelegramMCP.exe"
        } else {
            "TelegramMCP"
        });
    target
}

/// Path to the built `tg-hook` binary in the same dir.
pub fn tg_hook_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tg-hook"))
}

/// Render a minimal config TOML for the server with a single alias `target`.
pub fn make_server_config(api_base: &str, db: &std::path::Path, target_id: i64) -> String {
    let db = db.display().to_string().replace('\\', "/");
    format!(
        r#"
[bot]
token = "12345:fake"
api_base_url = "{api_base}"

[storage]
path = "{db}"

[updater]
enabled = false

[aliases]
target = {target_id}
"#
    )
}

/// Spawn `TelegramMCP --config <cfg>` with stdio piped so it runs to completion
/// as an MCP server. We don't speak MCP on its stdio in these tests — the hook
/// talks to it over the local pipe — so we just need to keep the process alive.
pub struct ServerHandle {
    pub child: Child,
}

impl ServerHandle {
    pub fn spawn(bin: &PathBuf, config: &PathBuf) -> Self {
        let child = Command::new(bin)
            .args(["--config", config.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn TelegramMCP");
        Self { child }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait for the server to write its discovery file. Polls every 100ms up
/// to `max_wait_ms`. Returns the parsed JSON or panics on timeout.
pub fn wait_for_discovery(
    expect_ppid: u32,
    max_wait_ms: u64,
) -> serde_json::Value {
    let base = dirs::data_local_dir().expect("LOCALAPPDATA");
    let dir = base.join("TelegramMCP").join("discovery");
    let mut waited = 0u64;
    while waited < max_wait_ms {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let Ok(raw) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                    continue;
                };
                if v.get("ppid").and_then(serde_json::Value::as_u64)
                    == Some(u64::from(expect_ppid))
                {
                    return v;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        waited += 100;
    }
    panic!("discovery file matching ppid={expect_ppid} did not appear within {max_wait_ms}ms");
}

/// Run the `tg-hook` binary with the given args. Returns its captured stdout
/// + exit status. `stdin_json` is piped to its stdin.
pub fn run_hook(args: &[&str], stdin_json: &str) -> (String, std::process::ExitStatus) {
    let mut child = Command::new(tg_hook_binary())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn tg-hook");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_json.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait hook");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    (stdout, output.status)
}
```

- [ ] **Step 2: Write `tests/e2e.rs`**

```rust
//! End-to-end: hook ↔ live TelegramMCP ↔ wiremock fake Bot API.

#![cfg(windows)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests panic on infra failures"
)]

mod common;

use rusqlite::{params, Connection};
use std::time::Duration;
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Direct-DB inbound insert. Mirrors what `tg-updater` would do for a real
/// incoming message — the schema columns are pulled from
/// `crates/history/src/schema.rs`.
fn insert_inbound(db: &std::path::Path, chat_id: i64, message_id: i64, date: i64, text: &str) {
    let c = Connection::open(db).unwrap();
    c.execute(
        "INSERT INTO chats(chat_id, kind, title, username, first_seen, last_seen) \
         VALUES (?1, 'private', NULL, NULL, ?2, ?2) \
         ON CONFLICT(chat_id) DO UPDATE SET last_seen=excluded.last_seen",
        params![chat_id, date],
    )
    .unwrap();
    c.execute(
        "INSERT INTO messages(chat_id, message_id, date, from_id, from_name, reply_to, \
                              text, media_kind, media_file_id, media_meta, direction, raw) \
         VALUES (?1, ?2, ?3, NULL, NULL, NULL, ?4, NULL, NULL, NULL, 'in', '{}')",
        params![chat_id, message_id, date, text],
    )
    .unwrap();
    c.execute(
        "INSERT INTO messages_fts(rowid, text) \
         SELECT rowid, text FROM messages WHERE chat_id=?1 AND message_id=?2",
        params![chat_id, message_id],
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn reply_path_returns_block_decision() {
    // 1. Fake Bot API: SendMessage returns message_id=100 in chat 42.
    let fake_bot = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 100,
                "date": 1_700_000_000,
                "chat": { "id": 42, "type": "private" }
            }
        })))
        .mount(&fake_bot)
        .await;

    // 2. Spawn the real TelegramMCP server pointing at the fake Bot API.
    let dir = tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let db = dir.path().join("h.db");
    std::fs::write(
        &cfg,
        common::make_server_config(&fake_bot.uri(), &db, 42),
    )
    .unwrap();
    let server = common::ServerHandle::spawn(&common::telegram_mcp_binary(), &cfg);

    // The discovery file's `ppid` is *this test process's* PID — the server
    // is our child. (Same trick works in production: Claude Code spawns the
    // server, so server.ppid == Claude Code's pid.)
    let _disco = common::wait_for_discovery(std::process::id(), 5_000);

    // 3. Simulate the user replying *before* the hook runs by inserting an
    //    inbound row with message_id > 100 (the wakeup we're about to send).
    //    The hook will send the wakeup (returns 100 from fake Bot API), then
    //    poll once and find this reply.
    insert_inbound(&db, 42, 101, 1_700_000_001, "yes ship it");

    // 4. Run the hook. Use short timeout/poll to keep the test snappy —
    //    the reply is already there, so it'll be found on the first poll.
    let (stdout, status) = common::run_hook(
        &[
            "--chat", "target",
            "--timeout-secs", "5",
            "--poll-secs", "1",
            "--message", "done — next?",
        ],
        r#"{"session_id":null,"stop_hook_active":false}"#,
    );

    assert!(status.success(), "hook exited non-zero: {status:?}");
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("hook stdout not JSON: {stdout:?} ({e})"));
    assert_eq!(v["decision"], "block");
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .contains("yes ship it"),
        "reason did not include reply text: {v:?}"
    );

    drop(server); // explicit reap

    // Give the OS a beat to clean the discovery file before the next test.
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_path_returns_retry_message() {
    let fake_bot = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 200,
                "date": 1_700_000_000,
                "chat": { "id": 42, "type": "private" }
            }
        })))
        .mount(&fake_bot)
        .await;

    let dir = tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let db = dir.path().join("h.db");
    std::fs::write(
        &cfg,
        common::make_server_config(&fake_bot.uri(), &db, 42),
    )
    .unwrap();
    let _server = common::ServerHandle::spawn(&common::telegram_mcp_binary(), &cfg);
    let _ = common::wait_for_discovery(std::process::id(), 5_000);

    // Don't insert any inbound. Hook should poll once or twice, hit timeout,
    // and return the retry message.
    let (stdout, status) = common::run_hook(
        &[
            "--chat", "target",
            "--timeout-secs", "2",
            "--poll-secs", "1",
            "--message", "still here?",
            "--retry-message", "no reply, retrying",
        ],
        r#"{}"#,
    );
    assert!(status.success());
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["decision"], "block");
    assert_eq!(v["reason"], "no reply, retrying");
}
```

- [ ] **Step 3: Add `rusqlite` as a dev-dep so the test can insert directly**

In `crates/tg-hook/Cargo.toml`, append to `[dev-dependencies]`:

```toml
rusqlite = { workspace = true }
wiremock = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
dirs = { workspace = true }
```

- [ ] **Step 4: Build the workspace so `TelegramMCP.exe` exists before the e2e runs**

The e2e test resolves the server binary by path, not via Cargo's `CARGO_BIN_EXE_*` (that only works for in-crate bins). Make sure both are built:

Run: `cargo build --workspace`
Expected: success.

- [ ] **Step 5: Run the e2e tests**

Run: `cargo test -p tg-hook --test e2e -- --test-threads=1`

`--test-threads=1` because both tests write a server-discovery file to the **shared** `%LOCALAPPDATA%\TelegramMCP\discovery\` directory under the test process's PPID, and running in parallel will produce ambiguous matches.

Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/tg-hook
git commit -m "test(tg-hook): e2e reply + timeout paths against live TelegramMCP"
```

---

## Milestone 6 — User docs + settings.json wiring

### Task 11: User-facing usage doc

**Files:**
- Create: `docs/claude-code-hook.md`
- Create: `crates/tg-hook/README.md` (short — links to the doc above)

- [ ] **Step 1: Write `docs/claude-code-hook.md`**

```markdown
# Claude Code Stop hook — `tg-hook`

`tg-hook` is a small Windows binary that wires Claude Code's `Stop` event to
your TelegramMCP server. When Claude finishes a turn, the hook sends a
wakeup message to a configured chat, waits for your reply on Telegram (up
to 60 min by default), and feeds the reply back into the Claude Code
session so the next turn happens automatically.

## Prerequisites

- TelegramMCP is configured and running inside Claude Code (i.e. you've got
  `tg_send_message` etc. working from the LLM).
- The bot has access to a chat you'll send wakeups to — and the chat is in
  `[access] allowed_send_targets` if you set that allowlist.

## Build

```powershell
cargo build --release -p tg-hook
```

The binary lands at `target\release\tg-hook.exe`.

## Configure the hook in `settings.json`

Add a `Stop` hook entry to your Claude Code project or user settings:

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "D:\\Work\\Programming\\MCP\\Telegram\\target\\release\\tg-hook.exe --chat alerts --message \"Claude finished. What's next?\" --timeout-secs 3600 --retry-message \"No reply within 60 min — waiting another round.\"",
            "timeout": 3700
          }
        ]
      }
    ]
  }
}
```

Notes:
- The Claude Code hook `timeout` (seconds) **must be larger** than the hook's
  own `--timeout-secs`, or Claude Code will kill the process before it
  gets a chance to retry.
- `--chat` accepts a numeric chat id (e.g. `-1001234567890`) or an alias
  defined in your TelegramMCP `config.toml` `[aliases]` table.

## How the loop feels

1. Claude finishes a turn.
2. The Claude Code UI displays `Waiting on keyboard interrupt or external message`.
3. Your Telegram chat receives the wakeup.
4. You reply on Telegram (anywhere — phone, desktop, web).
5. The hook returns your reply to Claude Code, which treats it as your
   next message. Claude works on it and the loop continues.

To **escape** the loop and let Claude Code stop normally, press **Ctrl+C**
inside Claude Code. The hook exits cleanly and your session ends as usual.

If you ignore the wakeup for 60 minutes:
- The hook returns the `--retry-message` to Claude.
- Claude responds and stops again.
- The hook fires again, sending a fresh wakeup message.
- This continues indefinitely until you either reply or Ctrl+C.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `no TelegramMCP discovery file matched` | The MCP server isn't running, or its discovery file got stale. | Make sure TelegramMCP is the configured MCP server for this Claude Code session. Check `%LOCALAPPDATA%\TelegramMCP\discovery\` — should contain one `.json` file per running server. |
| `N discovery files matched; cannot disambiguate` | Multiple TelegramMCP servers share the same PPID (e.g. two Claude Code sessions launched the same way) and `$env:CLAUDE_SESSION_ID` isn't set. | Newer Claude Code builds set `CLAUDE_SESSION_ID` automatically. Confirm via `Get-ChildItem env:CLAUDE_SESSION_ID` from inside a hook command. Older builds: close one of the sessions. |
| Hook never returns even after Telegram reply | The reply landed in a different chat than `--chat`. | Verify the chat id with `tg_history_list_chats`. The hook only watches the chat the wakeup was sent to. |
| Log lines aren't showing up | Hook logs go to file, not stderr. | Tail `%LOCALAPPDATA%\TelegramMCP\logs\tg-hook-<pid>.log`. Set `$env:TG_HOOK_LOG=debug` to bump verbosity. |
```

- [ ] **Step 2: Write `crates/tg-hook/README.md`**

```markdown
# tg-hook

Claude Code Stop hook for TelegramMCP. See [`docs/claude-code-hook.md`](../../docs/claude-code-hook.md) for setup and usage.
```

- [ ] **Step 3: Commit**

```bash
git add crates/tg-hook docs/claude-code-hook.md
git commit -m "docs(tg-hook): user-facing setup + troubleshooting guide"
```

---

## Self-review checklist (run before declaring done)

- [ ] `cargo fmt --all -- --check` is clean.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- [ ] `cargo test --workspace --all-targets` passes (run with `--test-threads=1` for the e2e file, which is the default `[test]` harness behavior unless you've overridden it).
- [ ] Manual smoke: start a real TelegramMCP server pointed at the real Telegram Bot API, run `tg-hook --chat <your-alias>`, reply on Telegram, observe `{"decision":"block","reason":"User replied on Telegram: ..."}` printed to stdout.
- [ ] Manual smoke (Ctrl+C path): start as above, press Ctrl+C in the terminal running `tg-hook`, observe exit code 0 with **no** JSON on stdout.
- [ ] Discovery selection prefers `session_id` when present (covered by `discover::tests::picks_by_session_id`, but also worth eyeballing the log line under `TG_HOOK_LOG=debug` to confirm the chosen PID matches what you expect).
