//! Local `SQLite`-backed history store for Telegram messages.

pub mod error;
mod schema;
mod store;
pub mod types;

pub use error::HistoryError;
pub use store::History;
pub use types::{ChatInfo, ChatKind, ChatSummary, Direction, SearchHit, StoredMessage};
