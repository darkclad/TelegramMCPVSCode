//! Error type for the outbound Telegram Bot API client.

use thiserror::Error;

/// Errors produced by [`crate::TgClient`] and related helpers.
#[derive(Debug, Error)]
pub enum TgClientError {
    /// Transport-level HTTP error (DNS, TCP, TLS, timeouts).
    ///
    /// Constructed only via [`crate::client::redact_reqwest_err`], which
    /// strips the request URL before wrapping — otherwise the bot token,
    /// which is embedded in `/bot<token>/...` paths, would leak through
    /// `reqwest::Error`'s `Display` impl into logs and MCP error responses.
    #[error("HTTP error: {0}")]
    Http(reqwest::Error),
    /// Telegram Bot API returned `ok: false` with an error code/description.
    #[error("Telegram API error {code}: {description}")]
    Api {
        /// Telegram API error code (`error_code` field in the response).
        code: i32,
        /// Human-readable description (`description` field in the response).
        description: String,
    },
    /// Telegram API rate-limited us; honour the `retry_after` hint.
    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited {
        /// Number of seconds to wait before retrying, per Telegram.
        retry_after_secs: u32,
    },
    /// A chat alias was not found in the alias map.
    #[error("unknown alias: {0}")]
    UnknownAlias(String),
    /// A chat reference (id, alias, username) could not be parsed/validated.
    #[error("invalid chat reference: {0}")]
    InvalidChat(String),
    /// The configured API base URL failed to parse.
    #[error("invalid api base URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    /// Error returned from the underlying `teloxide` request layer.
    #[error("teloxide error: {0}")]
    Teloxide(#[from] teloxide::RequestError),
    /// Failure while downloading a file from Telegram's file endpoint.
    #[error("file download error: {0}")]
    Download(String),
}

// `code()` is consumed by `mcp-server` error mapping in Task 17+; until then
// it is dead code in this crate.
#[allow(dead_code)]
impl TgClientError {
    /// Stable machine-readable error code suitable for MCP error responses.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Http(_) | Self::Download(_) => "http_error",
            Self::Api { .. } | Self::Teloxide(_) => "telegram_api_error",
            Self::RateLimited { .. } => "rate_limited",
            Self::UnknownAlias(_) => "unknown_alias",
            Self::InvalidChat(_) => "invalid_chat",
            Self::InvalidUrl(_) => "invalid_url",
        }
    }
}
