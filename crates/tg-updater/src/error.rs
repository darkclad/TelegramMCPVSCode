//! Error type for the [`tg-updater`](crate) crate.

use thiserror::Error;

/// Errors produced while polling Telegram updates and persisting them.
#[derive(Debug, Error)]
pub enum UpdaterError {
    /// Outbound Telegram client returned an error.
    #[error("client error: {0}")]
    Client(#[from] tg_client::TgClientError),
    /// The local history store returned an error.
    #[error("history error: {0}")]
    Store(#[from] history::HistoryError),
    /// An update payload could not be decoded as JSON.
    #[error("update decode: {0}")]
    Decode(#[from] serde_json::Error),
}
