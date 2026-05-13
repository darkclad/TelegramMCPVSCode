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

    /// Send a text message via Telegram Bot API `sendMessage`.
    ///
    /// `chat_id` is the numeric Telegram chat id. `text` is the message body
    /// (1..=4096 UTF-16 code units, per Telegram). `parse_mode` selects HTML
    /// / `MarkdownV2` formatting when supplied. `reply_to` makes the new
    /// message a reply to the given message id within the same chat.
    /// `silent` sends without a notification sound. `link_preview_enabled`
    /// controls whether Telegram generates a link preview for URLs in the
    /// message text; pass `false` to suppress it.
    ///
    /// # Errors
    ///
    /// Returns [`TgClientError::RateLimited`] when Telegram responds with a
    /// `retry_after` hint, [`TgClientError::Api`] for other API-level
    /// failures, and [`TgClientError::Teloxide`] for transport / parsing
    /// failures surfaced by the underlying `teloxide` request layer.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::fn_params_excessive_bools,
        reason = "API surface matches Telegram Bot API parameters"
    )]
    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<teloxide::types::ParseMode>,
        reply_to: Option<i64>,
        silent: bool,
        link_preview_enabled: bool,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        use teloxide::types::{LinkPreviewOptions, MessageId, ReplyParameters};

        let mut req = self.bot.send_message(ChatId(chat_id), text);
        if let Some(pm) = parse_mode {
            req = req.parse_mode(pm);
        }
        if let Some(rid) = reply_to {
            req = req.reply_parameters(ReplyParameters::new(MessageId(rid as i32)));
        }
        if silent {
            req = req.disable_notification(true);
        }
        if !link_preview_enabled {
            req = req.link_preview_options(LinkPreviewOptions {
                is_disabled: true,
                url: None,
                prefer_small_media: false,
                prefer_large_media: false,
                show_above_text: false,
            });
        }
        let msg = req.await.map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: i64::from(msg.id.0),
            date: msg.date.timestamp(),
        })
    }

    /// Edit the text of a previously sent message via `editMessageText`.
    ///
    /// `chat_id` / `message_id` identify the target message; `text` is the
    /// replacement body. `parse_mode` controls HTML / `MarkdownV2` rendering.
    ///
    /// # Errors
    ///
    /// See [`TgClient::send_message`] — error mapping is identical.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Telegram message IDs fit in i32"
    )]
    pub async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        parse_mode: Option<teloxide::types::ParseMode>,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        let mut req = self.bot.edit_message_text(
            ChatId(chat_id),
            teloxide::types::MessageId(message_id as i32),
            text,
        );
        if let Some(pm) = parse_mode {
            req = req.parse_mode(pm);
        }
        let msg = req.await.map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: i64::from(msg.id.0),
            date: msg.date.timestamp(),
        })
    }

    /// Delete a message in the given chat via `deleteMessage`.
    ///
    /// Telegram returns `true` on success; this method discards that and
    /// returns `()`.
    ///
    /// # Errors
    ///
    /// See [`TgClient::send_message`] — error mapping is identical.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Telegram message IDs fit in i32"
    )]
    pub async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<(), TgClientError> {
        use teloxide::prelude::*;
        self.bot
            .delete_message(ChatId(chat_id), teloxide::types::MessageId(message_id as i32))
            .await
            .map_err(map_teloxide_err)?;
        Ok(())
    }

    /// Forward a message from `from_chat` to `to_chat` via `forwardMessage`.
    ///
    /// Returns a [`SentMessage`] describing the newly created message in the
    /// destination chat — `chat_id` is `to_chat`, `message_id` is the new id.
    ///
    /// # Errors
    ///
    /// See [`TgClient::send_message`] — error mapping is identical.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "Telegram message IDs fit in i32"
    )]
    pub async fn forward_message(
        &self,
        from_chat: i64,
        message_id: i64,
        to_chat: i64,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        let msg = self
            .bot
            .forward_message(
                ChatId(to_chat),
                ChatId(from_chat),
                teloxide::types::MessageId(message_id as i32),
            )
            .await
            .map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: i64::from(msg.id.0),
            date: msg.date.timestamp(),
        })
    }

    /// Send a chat action (typing, `upload_photo`, …) via `sendChatAction`.
    ///
    /// Useful for surfacing "typing…" UI before a long-running response.
    ///
    /// # Errors
    ///
    /// See [`TgClient::send_message`] — error mapping is identical.
    pub async fn send_chat_action(
        &self,
        chat_id: i64,
        action: teloxide::types::ChatAction,
    ) -> Result<(), TgClientError> {
        use teloxide::prelude::*;
        self.bot
            .send_chat_action(ChatId(chat_id), action)
            .await
            .map_err(map_teloxide_err)?;
        Ok(())
    }

    /// Upload and send a local image file via `sendPhoto`.
    ///
    /// `path` points at an on-disk image; `caption` and `parse_mode` are
    /// optional. The upload is performed via multipart so this method
    /// requires real network I/O and is exercised in higher-level end-to-end
    /// tests rather than via `wiremock`.
    ///
    /// # Errors
    ///
    /// See [`TgClient::send_message`] — error mapping is identical.
    pub async fn send_photo_path(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        caption: Option<&str>,
        parse_mode: Option<teloxide::types::ParseMode>,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        use teloxide::types::InputFile;
        let mut req = self.bot.send_photo(ChatId(chat_id), InputFile::file(path));
        if let Some(c) = caption {
            req = req.caption(c);
        }
        if let Some(pm) = parse_mode {
            req = req.parse_mode(pm);
        }
        let msg = req.await.map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: i64::from(msg.id.0),
            date: msg.date.timestamp(),
        })
    }

    /// Upload and send a local file as a document via `sendDocument`.
    ///
    /// `path` points at an on-disk file; `caption` is optional; `filename`
    /// overrides the name Telegram displays. As with [`Self::send_photo_path`]
    /// the upload is multipart and is covered by higher-level tests.
    ///
    /// # Errors
    ///
    /// See [`TgClient::send_message`] — error mapping is identical.
    pub async fn send_document_path(
        &self,
        chat_id: i64,
        path: &std::path::Path,
        caption: Option<&str>,
        filename: Option<&str>,
    ) -> Result<SentMessage, TgClientError> {
        use teloxide::prelude::*;
        use teloxide::types::InputFile;
        let mut file = InputFile::file(path);
        if let Some(name) = filename {
            file = file.file_name(name.to_string());
        }
        let mut req = self.bot.send_document(ChatId(chat_id), file);
        if let Some(c) = caption {
            req = req.caption(c);
        }
        let msg = req.await.map_err(map_teloxide_err)?;
        Ok(SentMessage {
            chat_id: msg.chat.id.0,
            message_id: i64::from(msg.id.0),
            date: msg.date.timestamp(),
        })
    }
}

fn map_teloxide_err(e: teloxide::RequestError) -> TgClientError {
    use teloxide::RequestError as R;
    match e {
        R::RetryAfter(d) => TgClientError::RateLimited {
            retry_after_secs: d.seconds(),
        },
        R::Api(ref api) => TgClientError::Api {
            code: api_code(api),
            description: api.to_string(),
        },
        other => TgClientError::Teloxide(other),
    }
}

fn api_code(_e: &teloxide::ApiError) -> i32 {
    // teloxide doesn't expose the numeric code on every variant; pick a stable
    // sentinel for non-rate-limit API errors so the LLM has something to match.
    0
}
