//! Long-poll loop driving Telegram's `getUpdates`.
//!
//! [`Updater::run`] consumes updates one batch at a time, maps each entry
//! through [`crate::map_update`], applies the optional chat allow-list, and
//! persists the resulting `(ChatInfo, StoredMessage)` pairs into the
//! [`History`] store. After each successful batch the highest `update_id`
//! observed is persisted (as `last_update_id + 1`) under the `update_offset`
//! key so a restart resumes from the correct point. Transient client errors
//! trigger exponential backoff (1s → 30s cap); Telegram `429` responses
//! sleep for the server-supplied `retry_after`.

use crate::{map_update, UpdaterError};
use history::History;
use std::collections::HashSet;
use std::time::Duration;
use tg_client::TgClient;

/// Runtime configuration for the [`Updater`] loop.
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    /// Long-poll timeout, in seconds, passed to Telegram's `getUpdates`.
    pub poll_timeout_secs: u64,
    /// Telegram `Update` kinds to subscribe to (e.g. `message`, `edited_message`).
    pub allowed_update_kinds: Vec<String>,
    /// Optional allow-list of chat ids; when `Some`, updates from other chats are dropped.
    pub allowed_chats: Option<Vec<i64>>,
}

/// Telegram update poller: runs `getUpdates` and persists incoming messages.
pub struct Updater {
    /// Outbound Telegram Bot API client.
    pub client: TgClient,
    /// Local history store to which incoming messages are written.
    pub store: History,
    /// Polling configuration.
    pub config: UpdaterConfig,
}

impl Updater {
    /// Run the long-poll loop forever, persisting incoming updates to the
    /// configured [`History`] store.
    ///
    /// The loop:
    /// 1. Loads the prior `update_offset` from the kv store (or starts at 0).
    /// 2. Calls `getUpdates` with the configured timeout and allowed kinds.
    /// 3. Maps each update via [`crate::map_update`], drops updates whose
    ///    chat id is not in `allowed_chats` (when set), and persists the
    ///    rest via [`History::upsert_chat`] + [`History::insert_message`].
    /// 4. Writes `last_update_id + 1` back to the kv store as the new
    ///    `update_offset` so a restart resumes correctly.
    /// 5. On client errors, sleeps with exponential backoff (1s, doubling,
    ///    capped at 30s). On `RateLimited`, honours the server-supplied
    ///    `retry_after_secs`.
    ///
    /// The returned [`Result`] only ever resolves to its [`Err`] arm on a
    /// fatal kv-read failure during startup; otherwise the loop runs
    /// forever and callers should drive cancellation via task abort.
    #[allow(
        clippy::cognitive_complexity,
        reason = "single linear poll/persist/backoff loop; splitting harms readability"
    )]
    pub async fn run(self) -> Result<std::convert::Infallible, UpdaterError> {
        let allowed: Option<HashSet<i64>> = self
            .config
            .allowed_chats
            .as_ref()
            .map(|v| v.iter().copied().collect());
        // Default to `0` when no offset is persisted: Telegram treats this as
        // "start from oldest unconfirmed update", and sending it explicitly
        // makes the request body match deterministically in tests.
        let mut offset: Option<i64> = Some(
            self.store
                .kv_get("update_offset")
                .await?
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        );
        let mut backoff = Duration::from_secs(1);
        let kinds_owned: Vec<String> = self.config.allowed_update_kinds.clone();

        loop {
            let kinds_refs: Vec<&str> = kinds_owned.iter().map(String::as_str).collect();
            let kinds_arg = if kinds_refs.is_empty() {
                None
            } else {
                Some(kinds_refs.as_slice())
            };

            let result = self
                .client
                .get_updates_raw(offset, self.config.poll_timeout_secs, kinds_arg)
                .await;

            match result {
                Ok(updates) => {
                    backoff = Duration::from_secs(1);
                    let mut max_update_id: Option<i64> = None;
                    for u in &updates {
                        let id = u.get("update_id").and_then(serde_json::Value::as_i64);
                        if let Some(i) = id {
                            max_update_id = Some(max_update_id.map_or(i, |m| m.max(i)));
                        }
                        let Some((chat, msg)) = map_update(u) else {
                            continue;
                        };
                        if let Some(a) = &allowed {
                            if !a.contains(&chat.chat_id) {
                                tracing::debug!(chat_id = chat.chat_id, "dropped: disallowed chat");
                                continue;
                            }
                        }
                        if let Err(e) = self.store.upsert_chat(&chat).await {
                            tracing::error!(error = %e, "upsert_chat failed");
                            continue;
                        }
                        if let Err(e) = self.store.insert_message(&msg).await {
                            tracing::error!(error = %e, "insert_message failed");
                        }
                    }
                    if let Some(m) = max_update_id {
                        let next = m + 1;
                        if let Err(e) = self
                            .store
                            .kv_put("update_offset", &next.to_string())
                            .await
                        {
                            tracing::error!(error = %e, "persist offset failed");
                        } else {
                            offset = Some(next);
                        }
                    }
                }
                Err(tg_client::TgClientError::RateLimited { retry_after_secs }) => {
                    tracing::warn!(retry_after_secs, "rate limited by Telegram");
                    tokio::time::sleep(Duration::from_secs(u64::from(retry_after_secs))).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, ?backoff, "getUpdates failed; backing off");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}
