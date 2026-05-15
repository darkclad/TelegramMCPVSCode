//! Library surface for the `tg-hook` binary. Modules are exposed so
//! integration tests in `crates/tg-hook/tests/` can drive them.

#![cfg(windows)]

pub mod cli;
pub mod discovery;
pub mod mcp_client;
pub mod output;
pub mod poll;
pub mod stop_input;
pub mod wake;
