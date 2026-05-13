//! Shared test helpers for the `mcp-server` end-to-end smoke tests.
//!
//! Spawns the real `TelegramMCP` binary as a child process and drives it
//! via JSON-RPC over stdio. Each test owns its own [`McpClient`] which
//! kills and reaps the child on drop.

#![allow(
    dead_code,
    reason = "helpers are shared across smoke tests; not every test uses every method"
)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests panic on infra failures"
)]
#![allow(
    clippy::ptr_arg,
    reason = "signatures mirror the plan; callers already own PathBufs"
)]
#![allow(
    clippy::needless_pass_by_value,
    reason = "callers construct the JSON inline; ergonomic over micro-optimal"
)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// JSON-RPC client over the child binary's stdio.
pub struct McpClient {
    /// The running `TelegramMCP` process. Killed and reaped on drop.
    pub child: Child,
    /// Pipe into the child's stdin (line-delimited JSON-RPC requests).
    pub stdin: ChildStdin,
    /// Buffered reader over the child's stdout (line-delimited JSON-RPC responses).
    pub stdout: BufReader<ChildStdout>,
    /// Monotonic id allocator for outgoing requests.
    pub next_id: u64,
}

impl McpClient {
    /// Spawn the binary with `--config <config>` and capture its stdio.
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
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    /// Send a JSON-RPC request and block until the response with the
    /// matching id arrives. Notifications (no `id`) are ignored.
    pub fn send(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        });
        writeln!(self.stdin, "{req}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            if v.get("id").and_then(serde_json::Value::as_u64) == Some(id) {
                return v;
            }
            // notifications (no id) are ignored
        }
    }

    /// Drive the MCP `initialize` handshake and send the required
    /// `notifications/initialized` follow-up.
    pub fn initialize(&mut self) {
        let resp = self.send(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "smoke", "version": "0" }
            }),
        );
        assert!(resp["result"].is_object(), "initialize response: {resp}");
        // Required by MCP: notify the server that initialization is done.
        let notify = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        });
        writeln!(self.stdin, "{notify}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Convenience wrapper around `tools/call`.
    pub fn call_tool(&mut self, name: &str, args: serde_json::Value) -> serde_json::Value {
        self.send(
            "tools/call",
            serde_json::json!({
                "name": name, "arguments": args
            }),
        )
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Absolute path to the `TelegramMCP` binary built for this test target.
///
/// Cargo populates `CARGO_BIN_EXE_<bin-name>` at compile time for any bin
/// target the integration test depends on.
pub fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_TelegramMCP"))
}
