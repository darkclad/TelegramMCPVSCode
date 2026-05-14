//! Per-instance discovery file: lets hooks find the right pipe + auth token.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// JSON written to the discovery file at server startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    /// PID of this MCP server.
    pub pid: u32,
    /// PID of the process that spawned us (Claude Code). Hooks compare their
    /// own PPID against this to pick the right server when multiple Claude
    /// Code sessions are open.
    pub ppid: u32,
    /// Full named-pipe path the server is listening on.
    pub pipe: String,
    /// Per-instance auth token. Hook must send `AUTH <token>\n` as the very
    /// first bytes on a new connection.
    pub token: String,
    /// `CLAUDE_SESSION_ID` if Claude Code passes it as env var; otherwise null.
    /// Best-effort signal — `ppid` is the canonical match key.
    pub session_id: Option<String>,
    /// ISO-8601 timestamp of when this server started.
    pub started_at: String,
}

/// Directory holding all discovery files for this user.
///
/// Returns `%LOCALAPPDATA%\TelegramMCP\discovery` on Windows.
pub fn discovery_dir() -> std::io::Result<PathBuf> {
    let base = dirs::data_local_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not resolve %LOCALAPPDATA%",
        )
    })?;
    let dir = base.join("TelegramMCP").join("discovery");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Path to this PID's discovery file.
pub fn discovery_file_for(pid: u32) -> std::io::Result<PathBuf> {
    Ok(discovery_dir()?.join(format!("{pid}.json")))
}

/// Write the discovery record to disk.
pub fn write(record: &DiscoveryRecord) -> std::io::Result<()> {
    let path = discovery_file_for(record.pid)?;
    let json = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Remove this PID's discovery file. Best-effort; missing file is not an error.
pub fn remove(pid: u32) {
    if let Ok(path) = discovery_file_for(pid) {
        let _ = std::fs::remove_file(path);
    }
}
