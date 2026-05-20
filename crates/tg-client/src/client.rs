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
    // Used to build the `/bot<token>/...` URLs for the `getUpdates` and
    // `downloadFile` paths, which bypass teloxide's typed request layer.
    token: String,
    // One shared HTTP client: `reqwest::Client` owns a connection pool, so
    // reusing it keeps TCP/TLS connections alive across long-poll cycles.
    http: reqwest::Client,
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

/// Identity of the bot itself, as reported by Telegram `getMe`.
///
/// Returned by [`TgClient::get_me`]; useful for logging the configured bot
/// account at startup and surfacing the bot username to MCP clients.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BotIdentity {
    /// Numeric Telegram user id of the bot account.
    pub id: i64,
    /// `@username` of the bot, without the leading `@`. Always present for
    /// real bot accounts; modelled as `Option` to mirror the wire format.
    pub username: Option<String>,
    /// Display name (first name) of the bot account.
    pub first_name: String,
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
        // Build one HTTP client up front and reuse it for every raw
        // `getUpdates` / `downloadFile` call. The per-call read timeout that
        // varies for long-poll is applied per-request, not on the client.
        let http = reqwest::Client::builder()
            .build()
            .map_err(redact_reqwest_err)?;
        Ok(Self {
            bot,
            api_base: url,
            token,
            http,
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
            .delete_message(
                ChatId(chat_id),
                teloxide::types::MessageId(message_id as i32),
            )
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

    /// Fetch the bot's own identity via `getMe`.
    ///
    /// Returns the bot's numeric id, optional `@username`, and display name —
    /// useful for verifying credentials at startup and surfacing the bot
    /// account to MCP clients.
    ///
    /// # Errors
    ///
    /// See [`TgClient::send_message`] — error mapping is identical.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "Telegram user ids comfortably fit in i64 for the foreseeable future"
    )]
    pub async fn get_me(&self) -> Result<BotIdentity, TgClientError> {
        use teloxide::prelude::Requester;
        let me = self.bot.get_me().await.map_err(map_teloxide_err)?;
        Ok(BotIdentity {
            id: me.id.0 as i64,
            username: me.username.clone(),
            first_name: me.first_name.clone(),
        })
    }

    /// Long-poll Telegram `getUpdates`, returning each update as a raw
    /// [`serde_json::Value`].
    ///
    /// Bypasses teloxide's typed `Update` enum so newly-introduced update
    /// kinds flow through unchanged without bumping the `teloxide`
    /// dependency. `offset` is the standard Telegram acknowledgement cursor
    /// (`last_update_id + 1`); `timeout_secs` is the long-poll deadline in
    /// seconds (`0` for an immediate-return short-poll); `allowed_updates`
    /// optionally restricts which update kinds Telegram will deliver.
    ///
    /// The underlying HTTP request uses a connect/read timeout of
    /// `timeout_secs + 10` so the long-poll deadline always trips before
    /// the transport timeout.
    ///
    /// # Errors
    ///
    /// Returns [`TgClientError::RateLimited`] on HTTP 429,
    /// [`TgClientError::Api`] on any `ok: false` response (including a
    /// `code: -1` sentinel when `result` is missing or not an array), and
    /// [`TgClientError::Http`] for transport-level failures.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        reason = "Telegram error codes fit in i32; retry_after fits in u32"
    )]
    pub async fn get_updates_raw(
        &self,
        offset: Option<i64>,
        timeout_secs: u64,
        allowed_updates: Option<&[&str]>,
    ) -> Result<Vec<serde_json::Value>, TgClientError> {
        // Leading `/` makes this an absolute-path reference per RFC 3986,
        // so `bot12345:...` is not misread as a URI scheme. This mirrors the
        // URL construction teloxide-core uses for its method URLs. The
        // `GetUpdates` casing matches teloxide's method-URL convention and is
        // accepted by Telegram (method names are case-insensitive).
        let url = self
            .api_base
            .join(&format!("/bot{}/GetUpdates", self.token))?;
        let mut body = serde_json::Map::new();
        if let Some(o) = offset {
            body.insert("offset".into(), serde_json::json!(o));
        }
        body.insert("timeout".into(), serde_json::json!(timeout_secs));
        if let Some(kinds) = allowed_updates {
            body.insert("allowed_updates".into(), serde_json::json!(kinds));
        }
        // Per-request read timeout: `timeout_secs + 10` so the long-poll
        // deadline always trips before the transport timeout.
        let resp: serde_json::Value = self
            .http
            .post(url)
            .timeout(std::time::Duration::from_secs(timeout_secs + 10))
            .json(&serde_json::Value::Object(body))
            .send()
            .await
            .map_err(redact_reqwest_err)?
            .json()
            .await
            .map_err(redact_reqwest_err)?;
        if resp["ok"].as_bool() != Some(true) {
            let code = resp["error_code"].as_i64().unwrap_or(0) as i32;
            let desc = resp["description"].as_str().unwrap_or("").to_string();
            if code == 429 {
                let ra = resp["parameters"]["retry_after"].as_u64().unwrap_or(1) as u32;
                return Err(TgClientError::RateLimited {
                    retry_after_secs: ra,
                });
            }
            return Err(TgClientError::Api {
                code,
                description: desc,
            });
        }
        let arr = resp["result"]
            .as_array()
            .cloned()
            .ok_or_else(|| TgClientError::Api {
                code: -1,
                description: "result is not an array".into(),
            })?;
        Ok(arr)
    }

    /// Download a Telegram-hosted file by `file_id` to `dest` on disk.
    ///
    /// Performs the two-step Telegram Bot API download flow: a `getFile`
    /// call to translate `file_id` into the server-relative `file_path`,
    /// followed by a streamed `GET <api_base>/file/bot<token>/<file_path>`
    /// whose body bytes are written into `dest`. Missing parent directories
    /// of `dest` are created. The response body is consumed chunk-by-chunk
    /// via [`reqwest::Response::chunk`] so large media never fully buffer in
    /// memory. Returns the number of bytes written.
    ///
    /// # Errors
    ///
    /// Returns [`TgClientError::Api`] when `getFile` responds with
    /// `ok: false`, [`TgClientError::Download`] when the `file_path` is
    /// missing from a successful `getFile` response or when the local write
    /// fails, and [`TgClientError::Http`] for transport-level failures
    /// (including non-2xx responses on the file fetch).
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Telegram error codes fit in i32"
    )]
    pub async fn download_file(
        &self,
        file_id: &str,
        dest: &std::path::Path,
    ) -> Result<u64, TgClientError> {
        use tokio::io::AsyncWriteExt;

        // 1) getFile to learn the path. Leading `/` mirrors the Task 13
        // construction so `bot12345:...` is not misread as a URI scheme;
        // `GetFile` casing matches teloxide's method-URL convention.
        let get_file_url = self.api_base.join(&format!("/bot{}/GetFile", self.token))?;
        let resp: serde_json::Value = self
            .http
            .post(get_file_url)
            .json(&serde_json::json!({ "file_id": file_id }))
            .send()
            .await
            .map_err(redact_reqwest_err)?
            .json()
            .await
            .map_err(redact_reqwest_err)?;
        if resp["ok"].as_bool() != Some(true) {
            return Err(TgClientError::Api {
                code: resp["error_code"].as_i64().unwrap_or(0) as i32,
                description: resp["description"].as_str().unwrap_or("").into(),
            });
        }
        let file_path = resp["result"]["file_path"]
            .as_str()
            .ok_or_else(|| TgClientError::Download("missing file_path".into()))?;

        // 2) GET the file bytes. Stream chunks straight to disk so large
        // attachments don't fully buffer in memory.
        let dl_url = self
            .api_base
            .join(&format!("/file/bot{}/{}", self.token, file_path))?;
        let mut resp = self
            .http
            .get(dl_url)
            .send()
            .await
            .map_err(redact_reqwest_err)?
            .error_for_status()
            .map_err(redact_reqwest_err)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TgClientError::Download(e.to_string()))?;
        }
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| TgClientError::Download(e.to_string()))?;
        let mut total = 0_u64;
        while let Some(chunk) = resp.chunk().await.map_err(redact_reqwest_err)? {
            file.write_all(&chunk)
                .await
                .map_err(|e| TgClientError::Download(e.to_string()))?;
            total += chunk.len() as u64;
        }
        // `tokio::fs::File` buffers writes; flush so the bytes are on disk
        // before we return (a plain drop can lose the tail of the file).
        file.flush()
            .await
            .map_err(|e| TgClientError::Download(e.to_string()))?;
        Ok(total)
    }
}

/// Strip the request URL from a [`reqwest::Error`] before propagating, so
/// the bot token (embedded in `/bot<token>/...` paths) cannot leak via the
/// error's `Display` impl into logs or MCP error responses.
///
/// `reqwest::Error::without_url` consumes the error and returns a new one
/// whose `Display` no longer includes the URL — the canonical way to scrub
/// secrets from reqwest errors.
pub(crate) fn redact_reqwest_err(e: reqwest::Error) -> TgClientError {
    TgClientError::Http(e.without_url())
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
    // teloxide doesn't expose the numeric code on its `ApiError` variants.
    // Return the same `-1` "unknown code" sentinel `get_updates_raw` uses for
    // synthetic API errors, so the LLM sees one consistent value rather than
    // a misleading `0` (not a real Telegram error code).
    -1
}
