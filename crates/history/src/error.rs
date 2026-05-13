//! Error type for the [`history`](crate) crate.

use thiserror::Error;

/// Errors returned by the [`History`](crate::History) store.
#[derive(Debug, Error)]
pub enum HistoryError {
    /// A raw `SQLite` error bubbled up from `rusqlite`.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A schema migration step failed.
    #[error("schema migration from v{from} to v{to} failed: {source}")]
    Migration {
        /// Schema version the migration started from.
        from: u32,
        /// Schema version the migration was targeting.
        to: u32,
        /// Underlying `SQLite` error.
        source: rusqlite::Error,
    },
    /// The requested message was not present in the store.
    #[error("message {chat_id}/{message_id} not found")]
    NotFound {
        /// Telegram chat id.
        chat_id: i64,
        /// Telegram message id within the chat.
        message_id: i64,
    },
    /// Stored data could not be decoded into the expected shape.
    #[error("stored data corruption: {0}")]
    Corruption(String),
    /// JSON (de)serialization of a stored payload failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// A blocking task spawned via `tokio::task::spawn_blocking` failed to join.
    #[error("blocking task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}
