//! `TelegramMCP` — MCP server binary, stdio transport.
//!
//! Task 17 lays down the crate skeleton: configuration parsing in
//! [`config`], domain-error mapping in [`error`], and tool I/O types in
//! [`tools_io`]. Task 18 wires `rmcp` request dispatch on top of these
//! modules.

mod config;
mod error;
mod tools_io;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("TelegramMCP starting (skeleton — Task 18 wires rmcp)");
    Ok(())
}
