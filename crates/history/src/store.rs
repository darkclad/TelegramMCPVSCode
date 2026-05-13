//! The [`History`] store handle.

use crate::HistoryError;
use std::path::Path;

/// Handle to the local `SQLite`-backed message history store.
#[derive(Debug)]
pub struct History;

impl History {
    /// Open (creating if needed) the history database at `path`, running any
    /// pending schema migrations.
    pub fn open(_path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        unimplemented!("filled by Task 5")
    }
}
