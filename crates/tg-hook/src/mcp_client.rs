//! Tiny MCP JSON-RPC client over a Windows named pipe. Owns the connect
//! + AUTH handshake and one `initialize` + N `tools/call` requests.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::os::windows::io::AsRawHandle as _;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

/// Win32 `ERROR_PIPE_BUSY` — all server pipe instances are momentarily in use.
const ERROR_PIPE_BUSY: i32 = 231;
/// How many times to retry a busy pipe before giving up.
const PIPE_OPEN_RETRIES: u32 = 10;
/// Delay between busy-pipe retries.
const PIPE_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Connected, authenticated, MCP-initialized client.
pub struct McpClient {
    write: tokio::io::WriteHalf<NamedPipeClient>,
    read: BufReader<tokio::io::ReadHalf<NamedPipeClient>>,
    next_id: u64,
}

impl McpClient {
    /// Connect to `pipe_path`, verify the server's identity, send
    /// `AUTH <token>\n`, then drive the MCP `initialize` handshake.
    ///
    /// `expected_server_pid` is the `pid` from the discovery record; the
    /// pipe's actual server process id is checked against it so a stale or
    /// tampered record routing us to a different process is rejected.
    ///
    /// `ClientOptions::open` will fail with `ERROR_PIPE_BUSY` if all server
    /// instances are currently in use; we retry briefly before giving up so
    /// a momentary race during server accept-loop doesn't fail the hook.
    pub async fn connect(pipe_path: &str, token: &str, expected_server_pid: u32) -> Result<Self> {
        let client = open_with_retry(pipe_path).await?;
        // Confirm the pipe is served by the process named in the discovery
        // record before sending anything over it.
        let server_pid = local_pipe::process::named_pipe_server_pid(client.as_raw_handle())
            .context("querying named-pipe server pid")?;
        if server_pid != expected_server_pid {
            bail!(
                "pipe server pid {server_pid} does not match discovery record pid \
                 {expected_server_pid} — record may be stale or tampered"
            );
        }
        let (r, mut w) = tokio::io::split(client);
        w.write_all(format!("AUTH {token}\n").as_bytes())
            .await
            .context("writing AUTH line")?;
        let mut me = Self {
            write: w,
            read: BufReader::new(r),
            next_id: 1,
        };
        me.initialize().await?;
        Ok(me)
    }

    async fn initialize(&mut self) -> Result<()> {
        let _ = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "tg-hook", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;
        // Per MCP, the client follows `initialize` with the `notifications/
        // initialized` notification (no id, no response expected).
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.write
            .write_all(format!("{notif}\n").as_bytes())
            .await
            .context("sending notifications/initialized")?;
        Ok(())
    }

    /// Send a `tools/call` request with the given tool name and arguments.
    /// Returns the parsed `result` object on success, surfaces any
    /// `error` object as an Err.
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let resp = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        if let Some(err) = resp.get("error") {
            return Err(anyhow!("MCP tool error: {err}"));
        }
        resp.get("result")
            .cloned()
            .ok_or_else(|| anyhow!("MCP response missing result: {resp}"))
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        });
        self.write
            .write_all(format!("{req}\n").as_bytes())
            .await
            .context("writing request")?;
        // Skip any notifications (no id) until we see our matching response.
        loop {
            let mut line = String::new();
            let n = self
                .read
                .read_line(&mut line)
                .await
                .context("reading response line")?;
            if n == 0 {
                return Err(anyhow!("pipe closed mid-request"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(trimmed).context("parsing JSON-RPC response")?;
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(v);
            }
        }
    }
}

async fn open_with_retry(pipe_path: &str) -> Result<NamedPipeClient> {
    // Retry the "all pipe instances busy" race; bail on anything else.
    for _ in 0..PIPE_OPEN_RETRIES {
        match ClientOptions::new().open(pipe_path) {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(PIPE_RETRY_DELAY).await;
            }
            Err(e) => return Err(e).context(format!("opening pipe {pipe_path}")),
        }
    }
    Err(anyhow!(
        "pipe {pipe_path} stayed busy after {PIPE_OPEN_RETRIES} retries"
    ))
}

/// Extract `result.content[0].text` from an MCP `tools/call` result — the
/// standard text-content envelope. Both `poll` and `wake` decode tool
/// payloads nested inside this shape.
pub fn tool_result_text(result: &Value) -> Result<&str> {
    result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c0| c0.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("MCP result missing content[0].text"))
}
