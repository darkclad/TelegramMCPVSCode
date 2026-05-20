//! Windows named-pipe IPC for `TelegramMCP`.
//!
//! Lets local processes (e.g., Claude Code hook scripts) talk to a running
//! `TelegramMCP` server without spawning a new instance. Each MCP server
//! process listens on a per-PID named pipe; a discovery file records the
//! pipe path and a per-instance auth token so hooks can find and authenticate
//! to the right instance.

#![cfg(windows)]

mod auth;
pub mod discovery;
pub mod process;
mod security;
pub mod server;

pub use discovery::DiscoveryRecord;
pub use process::{pid_ancestry_chain, process_alive};
pub use server::{ConnHandler, PipeError, run_pipe_server};
