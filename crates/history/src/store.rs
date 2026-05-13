//! The [`History`] store handle.

use crate::{schema, HistoryError};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Handle to the local `SQLite`-backed message history store.
///
/// Cheap to clone — internally an [`Arc`] around the underlying connection.
#[derive(Debug, Clone)]
pub struct History {
    inner: Arc<Mutex<Connection>>,
}

impl History {
    /// Open (creating if needed) the history database at `path`, running any
    /// pending schema migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let mut conn = Connection::open(path)?;
        schema::migrate(&mut conn)?;
        Ok(Self { inner: Arc::new(Mutex::new(conn)) })
    }

    /// Return the current schema version recorded in the `kv` table.
    pub fn schema_version(&self) -> Result<u32, HistoryError> {
        let guard = self.inner.blocking_lock();
        let v: String = guard.query_row(
            "SELECT value FROM kv WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )?;
        v.parse()
            .map_err(|_| HistoryError::Corruption("schema_version not a number".into()))
    }

    #[allow(dead_code)] // wired up in Task 6+
    pub(crate) fn conn(&self) -> Arc<Mutex<Connection>> {
        self.inner.clone()
    }
}
