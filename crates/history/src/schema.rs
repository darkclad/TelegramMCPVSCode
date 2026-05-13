//! `SQLite` schema definition and migration runner.

use crate::HistoryError;
use rusqlite::{params, Connection};

pub(crate) const CURRENT_VERSION: u32 = 1;

const SCHEMA_V1: &str = r"
CREATE TABLE chats (
    chat_id    INTEGER PRIMARY KEY,
    kind       TEXT NOT NULL,
    title      TEXT,
    username   TEXT,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL
);

CREATE TABLE messages (
    chat_id       INTEGER NOT NULL,
    message_id    INTEGER NOT NULL,
    date          INTEGER NOT NULL,
    from_id       INTEGER,
    from_name     TEXT,
    reply_to      INTEGER,
    text          TEXT,
    media_kind    TEXT,
    media_file_id TEXT,
    media_meta    TEXT,
    direction     TEXT NOT NULL,
    raw           TEXT NOT NULL,
    PRIMARY KEY (chat_id, message_id)
);

CREATE INDEX idx_messages_chat_date ON messages(chat_id, date DESC);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    text,
    content='messages',
    content_rowid='rowid'
);

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

pub(crate) fn migrate(conn: &mut Connection) -> Result<u32, HistoryError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let current = read_version(conn)?;
    if current >= CURRENT_VERSION {
        return Ok(current);
    }

    let tx = conn.transaction()?;
    if current == 0 {
        tx.execute_batch(SCHEMA_V1)
            .map_err(|e| HistoryError::Migration { from: 0, to: 1, source: e })?;
        tx.execute(
            "INSERT OR REPLACE INTO kv(key, value) VALUES ('schema_version', ?1)",
            params!["1"],
        )?;
    }
    tx.commit()?;
    Ok(CURRENT_VERSION)
}

fn read_version(conn: &Connection) -> Result<u32, HistoryError> {
    // If kv doesn't exist yet, we're at v0.
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='kv'",
        [],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(0);
    }
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}
