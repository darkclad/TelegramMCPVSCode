//! Integration tests for `upsert_chat`, `insert_message`, `get_chat`,
//! `get_message` on [`History`].

use history::{ChatInfo, ChatKind, Direction, History, StoredMessage};
use serde_json::json;
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, History) {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    (dir, h)
}

fn sample_chat() -> ChatInfo {
    ChatInfo {
        chat_id: 100,
        kind: ChatKind::Private,
        title: Some("alice".into()),
        username: Some("alice".into()),
        first_seen: 1_000,
        last_seen: 1_000,
    }
}

fn sample_msg(message_id: i64, text: &str, direction: Direction) -> StoredMessage {
    StoredMessage {
        chat_id: 100,
        message_id,
        date: 1_000 + message_id,
        from_id: Some(100),
        from_name: Some("alice".into()),
        reply_to: None,
        text: Some(text.into()),
        media_kind: None,
        media_file_id: None,
        media_meta: None,
        direction,
        raw: json!({ "message_id": message_id }),
    }
}

#[tokio::test]
async fn upsert_chat_inserts_then_updates_last_seen() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    let got = h.get_chat(100).await.unwrap().unwrap();
    assert_eq!(got.last_seen, 1_000);

    let mut c = sample_chat();
    c.last_seen = 2_000;
    h.upsert_chat(&c).await.unwrap();
    let got2 = h.get_chat(100).await.unwrap().unwrap();
    assert_eq!(got2.last_seen, 2_000);
    // first_seen must NOT be overwritten on update
    assert_eq!(got2.first_seen, 1_000);
}

#[tokio::test]
async fn insert_message_roundtrip() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    h.insert_message(&sample_msg(1, "hello", Direction::In)).await.unwrap();
    let got = h.get_message(100, 1).await.unwrap();
    assert_eq!(got.text.as_deref(), Some("hello"));
    assert_eq!(got.direction, Direction::In);
}

#[tokio::test]
async fn insert_message_idempotent_on_duplicate_pk() {
    let (_d, h) = fresh();
    h.upsert_chat(&sample_chat()).await.unwrap();
    h.insert_message(&sample_msg(1, "hello", Direction::In)).await.unwrap();
    // Same (chat_id, message_id) but different text — should overwrite
    h.insert_message(&sample_msg(1, "edited", Direction::In)).await.unwrap();
    let got = h.get_message(100, 1).await.unwrap();
    assert_eq!(got.text.as_deref(), Some("edited"));
}

#[tokio::test]
async fn get_message_missing_returns_not_found() {
    let (_d, h) = fresh();
    let err = h.get_message(100, 999).await.unwrap_err();
    assert!(matches!(err, history::HistoryError::NotFound { .. }));
}
