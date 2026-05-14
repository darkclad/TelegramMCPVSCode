//! Tiny MCP JSON-RPC client over a Windows named pipe. Owns the connect
//! + AUTH handshake and one `initialize` + N `tools/call` requests.

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

/// Connected, authenticated, MCP-initialized client.
pub struct McpClient {
    write: tokio::io::WriteHalf<NamedPipeClient>,
    read: BufReader<tokio::io::ReadHalf<NamedPipeClient>>,
    next_id: u64,
}

impl McpClient {
    /// Connect to `pipe_path`, send `AUTH <token>\n`, then drive the MCP
    /// `initialize` handshake.
    ///
    /// `ClientOptions::open` will fail with `ERROR_PIPE_BUSY` if all server
    /// instances are currently in use; we retry briefly before giving up so
    /// a momentary race during server accept-loop doesn't fail the hook.
    pub async fn connect(pipe_path: &str, token: &str) -> Result<Self> {
        let client = open_with_retry(pipe_path).await?;
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
    // ERROR_PIPE_BUSY = 231.
    for _ in 0..10 {
        match ClientOptions::new().open(pipe_path) {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => return Err(e).context(format!("opening pipe {pipe_path}")),
        }
    }
    Err(anyhow!("pipe {pipe_path} stayed busy after 10 retries"))
}
