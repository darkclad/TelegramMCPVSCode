//! Shared helpers for the tg-hook e2e tests. Reuses the same wiremock +
//! tempdir pattern as `mcp-server/tests/common`.

#![allow(
    dead_code,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests panic on infra failures"
)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};

/// Return the path to the `TelegramMCP` binary.
///
/// `CARGO_BIN_EXE_TelegramMCP` is only populated by Cargo for integration
/// test targets that live in the *same crate* as the binary.  From a
/// cross-crate test we derive the path from the workspace layout instead.
pub fn telegrammcp_binary() -> PathBuf {
    // CARGO_MANIFEST_DIR is this crate's directory:
    //   …/crates/tg-hook
    // The workspace target directory is two levels up:
    //   …/target/debug/TelegramMCP[.exe]
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent() // crates/
        .and_then(Path::parent) // workspace root
        .expect("workspace root exists");

    // Respect an explicit CARGO_TARGET_DIR override, otherwise default to
    // <workspace_root>/target.
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map_or_else(|_| workspace_root.join("target"), PathBuf::from);

    let profile = std::env::var("CARGO_PROFILE_SELECTED").unwrap_or_else(|_| "debug".to_string());

    let mut bin = target_dir.join(profile).join("TelegramMCP");
    if cfg!(windows) {
        bin.set_extension("exe");
    }
    bin
}

pub fn tg_hook_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tg-hook"))
}

pub fn make_config(api_base: &str, db: &Path, alias_id: i64, session_id_marker: &str) -> String {
    let db = db.display().to_string().replace('\\', "/");
    // Note: we keep the updater disabled so we don't race the long-poll
    // background loop. The test seeds inbound rows directly into SQLite
    // and the hook reads them via tg_history_messages.
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
test = {alias_id}

[access]
allowed_send_targets = ["test"]

# Marker so this server's discovery record can be distinguished in tests:
# {session_id_marker}
"#
    )
}

/// A running `TelegramMCP` server, with its stdio MCP transport kept alive.
///
/// `TelegramMCP` serves MCP over stdio; if stdin reaches EOF the rmcp
/// runtime tears down and the process exits.  This guard holds the write
/// end of stdin open (after completing the MCP `initialize` handshake) so
/// the server stays alive for the duration of the test.
pub struct ServerGuard {
    /// The child process handle; killed on drop.
    pub child: Child,
    /// The write end of the child's stdin — kept alive to prevent EOF.
    pub _stdin: ChildStdin,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn `TelegramMCP` with the given config and `CLAUDE_SESSION_ID`, drive
/// the MCP `initialize` handshake on stdio so the server stays alive, and
/// return a [`ServerGuard`] that kills the child on drop.
pub fn spawn_server(cfg_path: &Path, session_id: &str) -> ServerGuard {
    let mut child = std::process::Command::new(telegrammcp_binary())
        .args(["--config", cfg_path.to_str().unwrap()])
        .env("CLAUDE_SESSION_ID", session_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn TelegramMCP");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = BufReader::new(stdout);

    // Drive the MCP initialize handshake so the rmcp transport is satisfied
    // and the server doesn't exit due to a missing initialize request.
    let init = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "tg-hook-e2e", "version": "0" }
        }
    });
    writeln!(stdin, "{init}").expect("write initialize");
    stdin.flush().expect("flush");

    // Read (and discard) the initialize response.
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read initialize response");

    // Send the required notifications/initialized follow-up.
    let notif = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    writeln!(stdin, "{notif}").expect("write notifications/initialized");
    stdin.flush().expect("flush");

    // We intentionally do NOT close stdin — keeping it open prevents EOF on
    // the server side and lets it continue serving named-pipe connections.
    // The ServerGuard holds _stdin so it is only dropped when the guard drops.
    ServerGuard {
        child,
        _stdin: stdin,
    }
}
