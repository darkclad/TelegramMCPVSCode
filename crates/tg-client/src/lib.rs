//! Outbound Telegram Bot API client.
//!
//! Wraps [`teloxide::Bot`] for typed API calls. The [`TgClient`] type is
//! stateless and cheaply cloneable; concrete send/get/download methods are
//! added by later tasks in the implementation plan.

mod client;
pub mod error;

pub use client::{BotIdentity, SentMessage, TgClient};
pub use error::TgClientError;
