//! Outbound Telegram Bot API client.
//!
//! Wraps [`teloxide::Bot`] for typed API calls. The [`TgClient`] type is
//! stateless and cheaply cloneable.

mod client;
pub mod error;

pub use client::{BotIdentity, SentMessage, TgClient};
pub use error::TgClientError;
