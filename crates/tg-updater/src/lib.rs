//! Telegram update poller: `getUpdates` → `history`.

pub mod error;
pub mod mapping;
mod polling;

pub use error::UpdaterError;
pub use mapping::map_update;
pub use polling::{Updater, UpdaterConfig};
