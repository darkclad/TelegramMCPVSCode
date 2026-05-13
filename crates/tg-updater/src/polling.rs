//! Long-poll loop driving `getUpdates`. Filled by Task 16.

use crate::UpdaterError;
use history::History;
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
    /// The returned [`Result`] only ever resolves to its [`Err`] arm; on
    /// success the loop never exits. Filled by Task 16.
    #[allow(
        clippy::unused_async,
        reason = "stub body; real `getUpdates` await chain lands in Task 16"
    )]
    pub async fn run(self) -> Result<std::convert::Infallible, UpdaterError> {
        unimplemented!("filled by Task 16")
    }
}
