//! Integration tests for the `kv` store, `mark_read`, and retention sweeps
//! (`trim_older_than`, `trim_per_chat_to`) on [`History`].

use history::{ChatInfo, ChatKind, Direction, History, StoredMessage};
use serde_json::json;
use tempfile::tempdir;

fn chat(id: i64) -> ChatInfo {
    ChatInfo {
        chat_id: id,
        kind: ChatKind::Private,
        title: None,
        username: None,
        first_seen: 0,
        last_seen: 0,
    }
}

fn msg(chat_id: i64, mid: i64, date: i64) -> StoredMessage {
    StoredMessage {
        chat_id,
        message_id: mid,
        date,
        from_id: None,
        from_name: None,
        reply_to: None,
        text: Some(format!("m{mid}")),
        media_kind: None,
        media_file_id: None,
        media_meta: None,
        direction: Direction::In,
        raw: json!({}),
    }
}

#[tokio::test]
async fn kv_roundtrip() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.kv_put("update_offset", "42").await.unwrap();
    assert_eq!(
        h.kv_get("update_offset").await.unwrap().as_deref(),
        Some("42")
    );
    assert_eq!(h.kv_get("missing").await.unwrap(), None);
}

#[tokio::test]
async fn mark_read_lowers_unread_count() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.upsert_chat(&chat(1)).await.unwrap();
    for i in 1..=5 {
        h.insert_message(&msg(1, i, i)).await.unwrap();
    }
    let before = &h.list_chats().await.unwrap()[0];
    assert_eq!(before.unread_count, 5);

    h.mark_read(1, 3).await.unwrap();
    let after = &h.list_chats().await.unwrap()[0];
    assert_eq!(after.unread_count, 2); // messages 4 and 5
}

#[tokio::test]
async fn retention_by_age_trims_old_messages() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.upsert_chat(&chat(1)).await.unwrap();
    // dates in seconds: 1 (old) and 1_000_000 (recent)
    h.insert_message(&msg(1, 1, 1)).await.unwrap();
    h.insert_message(&msg(1, 2, 1_000_000)).await.unwrap();
    // keep everything dated >= 999_000
    let removed = h.trim_older_than(999_000).await.unwrap();
    assert_eq!(removed, 1);
    let remaining = h.messages(1, None, None, 100).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].message_id, 2);
}

#[tokio::test]
async fn retention_by_count_keeps_newest_n_per_chat() {
    let dir = tempdir().unwrap();
    let h = History::open(dir.path().join("h.db")).unwrap();
    h.upsert_chat(&chat(1)).await.unwrap();
    for i in 1..=10 {
        h.insert_message(&msg(1, i, i)).await.unwrap();
    }
    let removed = h.trim_per_chat_to(3).await.unwrap();
    assert_eq!(removed, 7);
    let remaining = h.messages(1, None, None, 100).await.unwrap();
    let ids: Vec<i64> = remaining.iter().map(|m| m.message_id).collect();
    assert_eq!(ids, vec![10, 9, 8]);
}
