//! Integration tests for [`tg_updater::map_update`].

use serde_json::json;
use tg_updater::map_update;

#[test]
fn maps_private_text_message() {
    let u = json!({
        "update_id": 1,
        "message": {
            "message_id": 7,
            "date": 1000,
            "chat": { "id": 42, "type": "private", "first_name": "alice" },
            "from": { "id": 42, "is_bot": false, "first_name": "alice" },
            "text": "hello"
        }
    });
    let (chat, msg) = map_update(u).unwrap();
    assert_eq!(chat.chat_id, 42);
    assert_eq!(msg.text.as_deref(), Some("hello"));
    assert_eq!(msg.message_id, 7);
}

#[test]
fn maps_photo_uses_largest() {
    let u = json!({
        "update_id": 2,
        "message": {
            "message_id": 1, "date": 1,
            "chat": { "id": 1, "type": "private", "first_name": "x" },
            "photo": [
                { "file_id": "small", "file_size": 100 },
                { "file_id": "big", "file_size": 9999 }
            ]
        }
    });
    let (_, msg) = map_update(u).unwrap();
    assert_eq!(msg.media_kind.as_deref(), Some("photo"));
    assert_eq!(msg.media_file_id.as_deref(), Some("big"));
}

#[test]
fn ignores_callback_query() {
    let u = json!({
        "update_id": 3,
        "callback_query": { "id": "cb", "from": { "id": 1, "is_bot": false, "first_name": "x" } }
    });
    assert!(map_update(u).is_none());
}
