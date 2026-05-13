//! Integration tests for `History::search` (FTS5).

#![allow(clippy::cast_possible_wrap)] // test indices are tiny `usize`s, never wrap

use history::{ChatInfo, ChatKind, Direction, History, StoredMessage};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn fts_finds_matching_message() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.upsert_chat(&ChatInfo {
        chat_id: 1, kind: ChatKind::Private, title: None, username: None,
        first_seen: 0, last_seen: 0,
    }).await.unwrap();
    for (i, text) in ["lunch plans", "dinner tonight", "lunch break too"].iter().enumerate() {
        h.insert_message(&StoredMessage {
            chat_id: 1, message_id: (i + 1) as i64, date: i as i64,
            from_id: None, from_name: None, reply_to: None,
            text: Some((*text).into()),
            media_kind: None, media_file_id: None, media_meta: None,
            direction: Direction::In, raw: json!({}),
        }).await.unwrap();
    }
    let hits = h.search("lunch", None, None, None).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.snippet.to_lowercase().contains("lunch")));
}

#[tokio::test]
async fn search_scopes_by_chat_when_set() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    for chat_id in [1, 2] {
        h.upsert_chat(&ChatInfo {
            chat_id, kind: ChatKind::Private, title: None, username: None,
            first_seen: 0, last_seen: 0,
        }).await.unwrap();
        h.insert_message(&StoredMessage {
            chat_id, message_id: 1, date: 0,
            from_id: None, from_name: None, reply_to: None,
            text: Some("shared keyword".into()),
            media_kind: None, media_file_id: None, media_meta: None,
            direction: Direction::In, raw: json!({}),
        }).await.unwrap();
    }
    let hits = h.search("keyword", Some(1), None, None).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chat_id, 1);
}
