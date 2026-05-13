//! [`TgClient`] — the outbound Telegram Bot API client.

use crate::TgClientError;
use std::fmt;
use url::Url;

/// Outbound Telegram Bot API client.
///
/// Wraps a [`teloxide::Bot`] configured against a (possibly overridden) API
/// base URL. The client itself is stateless and cheaply cloneable; concrete
/// API call methods are added by subsequent tasks.
#[derive(Clone)]
pub struct TgClient {
    // Driven by typed send/get methods wired up in Task 11+.
    #[allow(dead_code)]
    bot: teloxide::Bot,
    api_base: Url,
    // Held for use by `getUpdates`/`downloadFile` paths wired up in Task 13+.
    #[allow(dead_code)]
    token: String,
}

impl fmt::Debug for TgClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TgClient")
            .field("api_base", &self.api_base.as_str())
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Minimal description of a message that was successfully sent to Telegram.
///
/// Returned by the various `send_*` methods added in later tasks.
#[derive(Debug, Clone)]
pub struct SentMessage {
    /// Chat identifier the message was delivered to.
    pub chat_id: i64,
    /// Telegram-assigned message identifier within the chat.
    pub message_id: i64,
    /// Unix timestamp (seconds) reported by Telegram for the sent message.
    pub date: i64,
}

impl TgClient {
    /// Construct a new client for the given bot token.
    ///
    /// `api_base` overrides the Telegram API root (default
    /// `https://api.telegram.org`); supplying a custom URL is primarily
    /// useful for test harnesses such as `wiremock`.
    pub fn new(token: String, api_base: Option<Url>) -> Result<Self, TgClientError> {
        let url = match api_base {
            Some(u) => u,
            None => "https://api.telegram.org"
                .parse()
                .expect("static URL parses"),
        };
        let bot = teloxide::Bot::new(token.clone()).set_api_url(url.clone());
        Ok(Self {
            bot,
            api_base: url,
            token,
        })
    }
}
