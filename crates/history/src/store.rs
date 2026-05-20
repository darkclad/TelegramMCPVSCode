//! The [`History`] store handle.

use crate::{HistoryError, schema};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Wrap a user-supplied search query as an FTS5 phrase so special chars
/// (-, :, *, ^, etc.) are treated as literal text. Internal double quotes
/// are escaped per FTS5 syntax.
fn fts_phrase_escape(s: &str) -> String {
    let escaped = s.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

/// Handle to the local `SQLite`-backed message history store.
///
/// Cheap to clone — internally an [`Arc`] around the underlying connection.
#[derive(Debug, Clone)]
pub struct History {
    inner: Arc<Mutex<Connection>>,
}

/// Insert-or-update a chat row on `conn`. Shared by [`History::upsert_chat`]
/// and [`History::record_inbound`]; `conn` may be a plain connection or a
/// transaction.
fn upsert_chat_sync(conn: &Connection, c: &crate::ChatInfo) -> Result<(), HistoryError> {
    conn.execute(
        "INSERT INTO chats(chat_id, kind, title, username, first_seen, last_seen) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(chat_id) DO UPDATE SET \
            kind=excluded.kind, title=excluded.title, username=excluded.username, \
            last_seen=excluded.last_seen",
        rusqlite::params![
            c.chat_id,
            c.kind.as_sql(),
            c.title,
            c.username,
            c.first_seen,
            c.last_seen
        ],
    )?;
    Ok(())
}

/// Insert-or-replace a message row on `conn` and keep the external-content
/// FTS5 index in sync. Shared by [`History::insert_message`] and
/// [`History::record_inbound`].
fn insert_message_sync(conn: &Connection, m: &crate::StoredMessage) -> Result<(), HistoryError> {
    let media_meta_s = m
        .media_meta
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let raw_s = serde_json::to_string(&m.raw)?;

    // External-content FTS5 requires explicit synchronization. Look up any
    // existing row first so we can issue the FTS5 'delete' command with the
    // OLD text before overwriting the content table.
    let existing: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT rowid, text FROM messages WHERE chat_id=?1 AND message_id=?2",
            rusqlite::params![m.chat_id, m.message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((old_rowid, old_text)) = existing {
        // FTS5 'delete' command syntax for external-content tables.
        conn.execute(
            "INSERT INTO messages_fts(messages_fts, rowid, text) VALUES('delete', ?1, ?2)",
            rusqlite::params![old_rowid, old_text],
        )?;
    }

    conn.execute(
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
            m.chat_id,
            m.message_id,
            m.date,
            m.from_id,
            m.from_name,
            m.reply_to,
            m.text,
            m.media_kind,
            m.media_file_id,
            media_meta_s,
            m.direction.as_sql(),
            raw_s,
        ],
    )?;
    // Index the (possibly new) row into FTS.
    conn.execute(
        "INSERT INTO messages_fts(rowid, text) \
         SELECT rowid, text FROM messages WHERE chat_id=?1 AND message_id=?2",
        rusqlite::params![m.chat_id, m.message_id],
    )?;
    Ok(())
}

impl History {
    /// Open (creating if needed) the history database at `path`, running any
    /// pending schema migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let mut conn = Connection::open(path)?;
        schema::migrate(&mut conn)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// Return the current schema version recorded in the `kv` table.
    pub fn schema_version(&self) -> Result<u32, HistoryError> {
        let guard = self.inner.blocking_lock();
        let v: String =
            guard.query_row("SELECT value FROM kv WHERE key='schema_version'", [], |r| {
                r.get(0)
            })?;
        v.parse()
            .map_err(|_| HistoryError::Corruption("schema_version not a number".into()))
    }

    /// Insert a chat row, or update the mutable fields (`kind`, `title`,
    /// `username`, `last_seen`) if a row with the same `chat_id` already
    /// exists. `first_seen` is preserved across updates.
    pub async fn upsert_chat(&self, c: &crate::ChatInfo) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        let c = c.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            upsert_chat_sync(&guard, &c)
        })
        .await?
    }

    /// Update `last_seen` on an existing chat row; does **not** create one.
    ///
    /// Used by the outbound mirror in `mcp-server`, which has no reliable way
    /// to know the chat kind for a send-only chat (e.g. an alerts channel
    /// the bot only posts to). The chat row is created on the first inbound
    /// update via [`Self::upsert_chat`], at which point the real
    /// [`crate::ChatKind`] is known; outbound sends just bump `last_seen`.
    ///
    /// Returns `Ok(())` whether or not a row matched — callers can rely on
    /// this being a no-op for chats not yet in the history.
    pub async fn touch_chat_last_seen(
        &self,
        chat_id: i64,
        last_seen: i64,
    ) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<(), HistoryError> {
            let guard = conn.blocking_lock();
            // `execute` returns rows-affected; we deliberately ignore it.
            guard.execute(
                "UPDATE chats SET last_seen = ?2 WHERE chat_id = ?1",
                rusqlite::params![chat_id, last_seen],
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
        tokio::task::spawn_blocking(move || {
            let guard = conn.blocking_lock();
            insert_message_sync(&guard, &m)
        })
        .await?
    }

    /// Persist an inbound `(chat, message)` pair in a single transaction.
    ///
    /// Folds [`Self::upsert_chat`] and [`Self::insert_message`] into one
    /// `spawn_blocking` round-trip and one `SQLite` commit — the updater
    /// calls this once per incoming message, so batching the two writes
    /// avoids a task hop and a separate WAL commit per message.
    pub async fn record_inbound(
        &self,
        chat: &crate::ChatInfo,
        msg: &crate::StoredMessage,
    ) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        let chat = chat.clone();
        let msg = msg.clone();
        tokio::task::spawn_blocking(move || -> Result<(), HistoryError> {
            let mut guard = conn.blocking_lock();
            let tx = guard.transaction()?;
            upsert_chat_sync(&tx, &chat)?;
            insert_message_sync(&tx, &msg)?;
            tx.commit()?;
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
                    let media_meta = media_meta_s.map(|s| serde_json::from_str(&s)).transpose()?;
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
                Err(rusqlite::Error::QueryReturnedNoRows) => Err(HistoryError::NotFound {
                    chat_id,
                    message_id,
                }),
                Err(e) => Err(HistoryError::Sqlite(e)),
            }
        })
        .await?
    }

    /// Fetch a page of messages from `chat_id`, newest first.
    ///
    /// The optional `before_message_id` and `after_message_id` cursors are
    /// both **exclusive** — they filter for `message_id < before` and
    /// `message_id > after` respectively. `limit` caps the page size.
    #[allow(clippy::too_many_lines)]
    // reason: SQL builder + row mapper is naturally long; helper extraction obscures the seam between them
    #[allow(clippy::similar_names)] // *_s suffixes mirror the SQL TEXT columns they decode
    pub async fn messages(
        &self,
        chat_id: i64,
        before_message_id: Option<i64>,
        after_message_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<crate::StoredMessage>, HistoryError> {
        use std::fmt::Write as _;
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<crate::StoredMessage>, HistoryError> {
            let guard = conn.blocking_lock();
            let mut sql = String::from(
                "SELECT message_id, date, from_id, from_name, reply_to, text, \
                            media_kind, media_file_id, media_meta, direction, raw \
                     FROM messages WHERE chat_id = ?1",
            );
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(chat_id)];
            if let Some(before) = before_message_id {
                write!(&mut sql, " AND message_id < ?{}", args.len() + 1).unwrap();
                args.push(Box::new(before));
            }
            if let Some(after) = after_message_id {
                write!(&mut sql, " AND message_id > ?{}", args.len() + 1).unwrap();
                args.push(Box::new(after));
            }
            write!(
                &mut sql,
                " ORDER BY message_id DESC LIMIT ?{}",
                args.len() + 1
            )
            .unwrap();
            args.push(Box::new(limit));

            let mut stmt = guard.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(AsRef::as_ref).collect();
            let iter = stmt.query_map(rusqlite::params_from_iter(param_refs), |r| {
                let dir_s: String = r.get(9)?;
                let media_meta_s: Option<String> = r.get(8)?;
                let raw_s: String = r.get(10)?;
                Ok((
                    r.get::<_, i64>(0)?, // message_id
                    r.get::<_, i64>(1)?, // date
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    media_meta_s,
                    dir_s,
                    raw_s,
                ))
            })?;

            let mut out = Vec::new();
            for row in iter {
                let (
                    mid,
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
                ) = row?;
                let direction = crate::Direction::from_sql(&dir_s)
                    .ok_or_else(|| HistoryError::Corruption(format!("bad direction: {dir_s}")))?;
                let media_meta = media_meta_s.map(|s| serde_json::from_str(&s)).transpose()?;
                let raw = serde_json::from_str(&raw_s)?;
                out.push(crate::StoredMessage {
                    chat_id,
                    message_id: mid,
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
                });
            }
            Ok(out)
        })
        .await?
    }

    /// List all known chats, newest-`last_seen` first, with the most recent
    /// stored `message_id` and a count of unread inbound messages.
    ///
    /// "Unread" is defined as inbound messages with `message_id` strictly
    /// greater than the per-chat `last_unread_baseline:<chat_id>` value stored
    /// in the `kv` table. When no baseline is recorded, the baseline is
    /// treated as `0` so every inbound message counts as unread.
    #[allow(clippy::cognitive_complexity)]
    // reason: linear collect + per-chat aggregation in one place is easier to read than two methods
    pub async fn list_chats(&self) -> Result<Vec<crate::ChatSummary>, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<crate::ChatSummary>, HistoryError> {
            let guard = conn.blocking_lock();
            // 1) Every chat in one query.
            let chats: Vec<(crate::ChatInfo, Option<i64>)> = {
                let mut stmt = guard.prepare(
                    "SELECT c.chat_id, c.kind, c.title, c.username, c.first_seen, c.last_seen, \
                            (SELECT MAX(message_id) FROM messages m WHERE m.chat_id=c.chat_id) \
                                AS last_msg \
                     FROM chats c \
                     ORDER BY c.last_seen DESC",
                )?;
                stmt.query_map([], |r| {
                    let kind_s: String = r.get(1)?;
                    Ok((
                        crate::ChatInfo {
                            chat_id: r.get(0)?,
                            kind: crate::ChatKind::from_sql(&kind_s)
                                .ok_or(rusqlite::Error::InvalidQuery)?,
                            title: r.get(2)?,
                            username: r.get(3)?,
                            first_seen: r.get(4)?,
                            last_seen: r.get(5)?,
                        },
                        r.get::<_, Option<i64>>(6)?,
                    ))
                })?
                .collect::<Result<_, _>>()?
            };

            // 2) Every unread baseline in one query — avoids a `kv` lookup
            //    per chat.
            let mut baselines: std::collections::HashMap<i64, i64> =
                std::collections::HashMap::new();
            {
                let mut bstmt = guard
                    .prepare("SELECT key, value FROM kv WHERE key LIKE 'last_unread_baseline:%'")?;
                let rows = bstmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                for row in rows {
                    let (key, value) = row?;
                    if let Some(id_str) = key.strip_prefix("last_unread_baseline:") {
                        if let (Ok(cid), Ok(base)) = (id_str.parse::<i64>(), value.parse::<i64>()) {
                            baselines.insert(cid, base);
                        }
                    }
                }
            }

            // 3) Per-chat unread count, reusing one prepared statement.
            let mut count_stmt = guard.prepare(
                "SELECT COUNT(*) FROM messages \
                 WHERE chat_id=?1 AND direction='in' AND message_id > ?2",
            )?;
            let mut out = Vec::with_capacity(chats.len());
            for (info, last_message_id) in chats {
                let baseline = baselines.get(&info.chat_id).copied().unwrap_or(0);
                let unread_count: i64 = count_stmt
                    .query_row(rusqlite::params![info.chat_id, baseline], |r| r.get(0))?;
                out.push(crate::ChatSummary {
                    info,
                    unread_count,
                    last_message_id,
                });
            }
            Ok(out)
        })
        .await?
    }

    /// Full-text search the message store, returning up to 100 hits ordered
    /// newest-first.
    ///
    /// `query` is passed to FTS5's `MATCH` operator, so callers can use the
    /// full FTS5 query syntax (prefix, phrase, boolean, ...). The optional
    /// `chat_id` restricts results to a single chat; `since` and `until` are
    /// inclusive unix-timestamp bounds on the message `date`. Each
    /// [`crate::SearchHit`] carries a `snippet` highlighting the match with
    /// `[` ... `]` delimiters and `…` ellipses.
    pub async fn search(
        &self,
        query: &str,
        chat_id: Option<i64>,
        since: Option<i64>,
        until: Option<i64>,
    ) -> Result<Vec<crate::SearchHit>, HistoryError> {
        use std::fmt::Write as _;
        let conn = self.inner.clone();
        let q = fts_phrase_escape(query);
        tokio::task::spawn_blocking(move || -> Result<Vec<crate::SearchHit>, HistoryError> {
            let guard = conn.blocking_lock();
            let mut sql = String::from(
                "SELECT m.chat_id, m.message_id, m.date, \
                        snippet(messages_fts, 0, '[', ']', '…', 10) AS snip \
                 FROM messages_fts \
                 JOIN messages m ON m.rowid = messages_fts.rowid \
                 WHERE messages_fts MATCH ?1",
            );
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(q)];
            if let Some(c) = chat_id {
                write!(&mut sql, " AND m.chat_id = ?{}", args.len() + 1).unwrap();
                args.push(Box::new(c));
            }
            if let Some(s) = since {
                write!(&mut sql, " AND m.date >= ?{}", args.len() + 1).unwrap();
                args.push(Box::new(s));
            }
            if let Some(u) = until {
                write!(&mut sql, " AND m.date <= ?{}", args.len() + 1).unwrap();
                args.push(Box::new(u));
            }
            sql.push_str(" ORDER BY m.date DESC LIMIT 100");

            let mut stmt = guard.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(AsRef::as_ref).collect();
            let hits = stmt
                .query_map(rusqlite::params_from_iter(param_refs), |r| {
                    Ok(crate::SearchHit {
                        chat_id: r.get(0)?,
                        message_id: r.get(1)?,
                        date: r.get(2)?,
                        snippet: r.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(hits)
        })
        .await?
    }

    /// Upsert a string value into the `kv` table under `key`.
    ///
    /// Used for small bits of persistent state (e.g. the tg-updater's
    /// `update_offset`, per-chat unread baselines, the schema version).
    pub async fn kv_put(&self, key: &str, value: &str) -> Result<(), HistoryError> {
        let conn = self.inner.clone();
        let key = key.to_string();
        let value = value.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), HistoryError> {
            let guard = conn.blocking_lock();
            guard.execute(
                "INSERT INTO kv(key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![key, value],
            )?;
            Ok(())
        })
        .await?
    }

    /// Read a string value from the `kv` table. Returns `Ok(None)` when no
    /// row with the given `key` exists.
    pub async fn kv_get(&self, key: &str) -> Result<Option<String>, HistoryError> {
        let conn = self.inner.clone();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<String>, HistoryError> {
            let guard = conn.blocking_lock();
            let row: Result<String, _> = guard.query_row(
                "SELECT value FROM kv WHERE key=?1",
                rusqlite::params![key],
                |r| r.get(0),
            );
            match row {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(HistoryError::Sqlite(e)),
            }
        })
        .await?
    }

    /// Mark all inbound messages in `chat_id` with `message_id <=
    /// up_to_message_id` as read by writing the baseline into the `kv` table.
    ///
    /// Consumed by [`Self::list_chats`] when computing `unread_count`.
    pub async fn mark_read(&self, chat_id: i64, up_to_message_id: i64) -> Result<(), HistoryError> {
        self.kv_put(
            &format!("last_unread_baseline:{chat_id}"),
            &up_to_message_id.to_string(),
        )
        .await
    }

    /// Delete all messages with `date < cutoff_unix_secs` and rebuild the FTS5
    /// index. Returns the number of rows deleted.
    ///
    /// The FTS5 `'rebuild'` command is used (rather than per-row `'delete'`)
    /// because it's the canonical way to keep an external-content FTS5 table
    /// in sync after a bulk DELETE.
    pub async fn trim_older_than(&self, cutoff_unix_secs: i64) -> Result<usize, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, HistoryError> {
            let guard = conn.blocking_lock();
            let removed = guard.execute(
                "DELETE FROM messages WHERE date < ?1",
                rusqlite::params![cutoff_unix_secs],
            )?;
            guard.execute(
                "INSERT INTO messages_fts(messages_fts) VALUES('rebuild')",
                [],
            )?;
            Ok(removed)
        })
        .await?
    }

    /// Per-chat retention: keep only the `keep_newest` highest-`message_id`
    /// rows in each chat, deleting older ones, then rebuild the FTS5 index.
    /// Returns the total number of rows deleted across all chats.
    ///
    /// Uses SQL window functions (`ROW_NUMBER() OVER (PARTITION BY ...)`),
    /// which require `SQLite` 3.25+ — provided by the bundled rusqlite.
    pub async fn trim_per_chat_to(&self, keep_newest: i64) -> Result<usize, HistoryError> {
        let conn = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, HistoryError> {
            let guard = conn.blocking_lock();
            let removed = guard.execute(
                "DELETE FROM messages WHERE rowid IN ( \
                    SELECT rowid FROM ( \
                        SELECT rowid, ROW_NUMBER() OVER ( \
                            PARTITION BY chat_id ORDER BY message_id DESC \
                        ) AS rn FROM messages \
                    ) WHERE rn > ?1 \
                 )",
                rusqlite::params![keep_newest],
            )?;
            guard.execute(
                "INSERT INTO messages_fts(messages_fts) VALUES('rebuild')",
                [],
            )?;
            Ok(removed)
        })
        .await?
    }
}
