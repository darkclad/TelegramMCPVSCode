//! The [`History`] store handle.

use crate::{schema, HistoryError};
use rusqlite::{Connection, OptionalExtension};
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

    /// Insert a chat row, or update the mutable fields (`kind`, `title`,
    /// `username`, `last_seen`) if a row with the same `chat_id` already
    /// exists. `first_seen` is preserved across updates.
    pub async fn upsert_chat(&self, c: &crate::ChatInfo) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        let c = c.clone();
        tokio::task::spawn_blocking(move || -> Result<(), HistoryError> {
            let guard = conn.blocking_lock();
            guard.execute(
                "INSERT INTO chats(chat_id, kind, title, username, first_seen, last_seen) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(chat_id) DO UPDATE SET \
                    kind=excluded.kind, title=excluded.title, username=excluded.username, \
                    last_seen=excluded.last_seen",
                rusqlite::params![
                    c.chat_id, c.kind.as_sql(), c.title, c.username, c.first_seen, c.last_seen
                ],
            )?;
            Ok(())
        })
        .await?
    }

    /// Look up a chat by its Telegram `chat_id`. Returns `Ok(None)` when no
    /// such row exists.
    pub async fn get_chat(&self, chat_id: i64) -> Result<Option<crate::ChatInfo>, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<crate::ChatInfo>, HistoryError> {
            let guard = conn.blocking_lock();
            let row = guard.query_row(
                "SELECT chat_id, kind, title, username, first_seen, last_seen \
                 FROM chats WHERE chat_id=?1",
                rusqlite::params![chat_id],
                |r| {
                    let kind_s: String = r.get(1)?;
                    Ok(crate::ChatInfo {
                        chat_id: r.get(0)?,
                        kind: crate::ChatKind::from_sql(&kind_s)
                            .ok_or(rusqlite::Error::InvalidQuery)?,
                        title: r.get(2)?,
                        username: r.get(3)?,
                        first_seen: r.get(4)?,
                        last_seen: r.get(5)?,
                    })
                },
            );
            match row {
                Ok(c) => Ok(Some(c)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(HistoryError::Sqlite(e)),
            }
        })
        .await?
    }

    /// Insert a message row, overwriting any existing row with the same
    /// `(chat_id, message_id)` primary key, and synchronize the
    /// external-content FTS5 index.
    pub async fn insert_message(&self, m: &crate::StoredMessage) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        let m = m.clone();
        tokio::task::spawn_blocking(move || -> Result<(), HistoryError> {
            let guard = conn.blocking_lock();
            let media_meta_s = m
                .media_meta
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            let raw_s = serde_json::to_string(&m.raw)?;

            // External-content FTS5 requires explicit synchronization. Look
            // up any existing row first so we can issue the FTS5 'delete'
            // command with the OLD text before overwriting the content table.
            let existing: Option<(i64, Option<String>)> = guard
                .query_row(
                    "SELECT rowid, text FROM messages \
                     WHERE chat_id=?1 AND message_id=?2",
                    rusqlite::params![m.chat_id, m.message_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((old_rowid, old_text)) = existing {
                // FTS5 'delete' command syntax for external-content tables.
                guard.execute(
                    "INSERT INTO messages_fts(messages_fts, rowid, text) \
                     VALUES('delete', ?1, ?2)",
                    rusqlite::params![old_rowid, old_text],
                )?;
            }

            guard.execute(
                "INSERT INTO messages(\
                    chat_id, message_id, date, from_id, from_name, reply_to, \
                    text, media_kind, media_file_id, media_meta, direction, raw) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                 ON CONFLICT(chat_id, message_id) DO UPDATE SET \
                    date=excluded.date, from_id=excluded.from_id, from_name=excluded.from_name, \
                    reply_to=excluded.reply_to, text=excluded.text, \
                    media_kind=excluded.media_kind, media_file_id=excluded.media_file_id, \
                    media_meta=excluded.media_meta, direction=excluded.direction, \
                    raw=excluded.raw",
                rusqlite::params![
                    m.chat_id, m.message_id, m.date, m.from_id, m.from_name, m.reply_to,
                    m.text, m.media_kind, m.media_file_id, media_meta_s,
                    m.direction.as_sql(), raw_s,
                ],
            )?;
            // Index the (possibly new) row into FTS.
            guard.execute(
                "INSERT INTO messages_fts(rowid, text) \
                 SELECT rowid, text FROM messages WHERE chat_id=?1 AND message_id=?2",
                rusqlite::params![m.chat_id, m.message_id],
            )?;
            Ok(())
        })
        .await?
    }

    /// Fetch the message identified by `(chat_id, message_id)`. Returns
    /// [`HistoryError::NotFound`] when no such row exists.
    #[allow(clippy::too_many_lines)] // straight-line column unpacking — splitting hurts readability
    #[allow(clippy::similar_names)] // *_s suffixes mirror the SQL TEXT columns they decode
    pub async fn get_message(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<crate::StoredMessage, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<crate::StoredMessage, HistoryError> {
            let guard = conn.blocking_lock();
            let row = guard.query_row(
                "SELECT date, from_id, from_name, reply_to, text, media_kind, media_file_id, \
                        media_meta, direction, raw \
                 FROM messages WHERE chat_id=?1 AND message_id=?2",
                rusqlite::params![chat_id, message_id],
                |r| {
                    let dir_s: String = r.get(8)?;
                    let media_meta_s: Option<String> = r.get(7)?;
                    let raw_s: String = r.get(9)?;
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        media_meta_s,
                        dir_s,
                        raw_s,
                    ))
                },
            );
            match row {
                Ok((
                    date,
                    from_id,
                    from_name,
                    reply_to,
                    text,
                    media_kind,
                    media_file_id,
                    media_meta_s,
                    dir_s,
                    raw_s,
                )) => {
                    let direction = crate::Direction::from_sql(&dir_s).ok_or_else(|| {
                        HistoryError::Corruption(format!("bad direction: {dir_s}"))
                    })?;
                    let media_meta = media_meta_s
                        .map(|s| serde_json::from_str(&s))
                        .transpose()?;
                    let raw = serde_json::from_str(&raw_s)?;
                    Ok(crate::StoredMessage {
                        chat_id,
                        message_id,
                        date,
                        from_id,
                        from_name,
                        reply_to,
                        text,
                        media_kind,
                        media_file_id,
                        media_meta,
                        direction,
                        raw,
                    })
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    Err(HistoryError::NotFound { chat_id, message_id })
                }
                Err(e) => Err(HistoryError::Sqlite(e)),
            }
        })
        .await?
    }
}
