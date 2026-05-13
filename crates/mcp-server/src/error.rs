//! Mapping helpers from internal domain errors to [`rmcp::Error`].
//!
//! Each tool implementation in Task 18 calls these to convert a typed crate
//! error into the right JSON-RPC error variant for the MCP wire protocol.
//! Errors that are clearly the caller's fault (bad alias, invalid chat) map
//! to `invalid_params`; transport and storage failures map to
//! `internal_error`.

// These helpers are wired in by the tool dispatch added in Task 18.
#![allow(dead_code, reason = "consumed by tool dispatch added in Task 18")]

use rmcp::Error as McpError;

/// Translate a [`tg_client::TgClientError`] into an [`McpError`].
pub fn client_err_to_mcp(e: &tg_client::TgClientError) -> McpError {
    use tg_client::TgClientError as E;
    let msg = e.to_string();
    match e {
        E::Http(_) | E::Teloxide(_) | E::Download(_) => McpError::internal_error(msg, None),
        E::Api { .. }
        | E::RateLimited { .. }
        | E::UnknownAlias(_)
        | E::InvalidChat(_)
        | E::InvalidUrl(_) => McpError::invalid_params(msg, None),
    }
}

/// Translate a [`history::HistoryError`] into an [`McpError`].
pub fn history_err_to_mcp(e: &history::HistoryError) -> McpError {
    use history::HistoryError as E;
    let msg = e.to_string();
    match e {
        E::NotFound { .. } | E::Corruption(_) => McpError::invalid_params(msg, None),
        _ => McpError::internal_error(msg, None),
    }
}

/// Translate an [`aliases::UnknownAlias`] into an [`McpError`].
pub fn alias_err_to_mcp(e: &aliases::UnknownAlias) -> McpError {
    McpError::invalid_params(e.to_string(), None)
}
