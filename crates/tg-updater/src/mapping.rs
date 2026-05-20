//! Pure JSON → `(ChatInfo, StoredMessage)` mapping for Telegram updates.

use history::{ChatInfo, ChatKind, Direction, StoredMessage};
use serde_json::Value;

/// Convert a Telegram `Update` JSON into a (`ChatInfo`, `StoredMessage`) pair
/// representing the message we should persist. Returns `None` for updates
/// that don't carry a storable message (callback queries, etc.).
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "single cohesive mapping; splitting it harms readability"
)]
pub fn map_update(update: Value) -> Option<(ChatInfo, StoredMessage)> {
    let msg = update
        .get("message")
        .or_else(|| update.get("edited_message"))
        .or_else(|| update.get("channel_post"))
        .or_else(|| update.get("edited_channel_post"))?;
    let chat = msg.get("chat")?;

    let chat_id = chat.get("id")?.as_i64()?;
    let kind = match chat.get("type")?.as_str()? {
        "private" => ChatKind::Private,
        "group" => ChatKind::Group,
        "supergroup" => ChatKind::Supergroup,
        "channel" => ChatKind::Channel,
        _ => return None,
    };
    let title = chat
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| full_name(chat));
    let username = chat
        .get("username")
        .and_then(Value::as_str)
        .map(str::to_string);

    let message_id = msg.get("message_id")?.as_i64()?;
    let date = msg.get("date")?.as_i64()?;
    let from = msg.get("from");
    let from_id = from.and_then(|f| f.get("id")).and_then(Value::as_i64);
    let from_name = from.and_then(full_name);
    let reply_to = msg
        .get("reply_to_message")
        .and_then(|r| r.get("message_id"))
        .and_then(Value::as_i64);
    let text = msg
        .get("text")
        .or_else(|| msg.get("caption"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let (media_kind, media_file_id, media_meta) = extract_media(msg);

    let chat_info = ChatInfo {
        chat_id,
        kind,
        title,
        username,
        first_seen: date,
        last_seen: date,
    };
    let stored = StoredMessage {
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
        direction: Direction::In,
        raw: update,
    };
    Some((chat_info, stored))
}

/// Combine `first_name` + `last_name` from a JSON object (a `chat` or a
/// `from`) into a display name, returning `None` when both are absent/empty.
fn full_name(obj: &Value) -> Option<String> {
    let first = obj.get("first_name").and_then(Value::as_str).unwrap_or("");
    let last = obj.get("last_name").and_then(Value::as_str).unwrap_or("");
    let combined = format!("{first} {last}");
    let trimmed = combined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn extract_media(msg: &Value) -> (Option<String>, Option<String>, Option<Value>) {
    for (key, kind) in [
        ("photo", "photo"),
        ("document", "document"),
        ("voice", "voice"),
        ("video", "video"),
        ("animation", "animation"),
        ("audio", "audio"),
        ("sticker", "sticker"),
    ] {
        if let Some(v) = msg.get(key) {
            // photos come as arrays; pick the largest by file_size.
            let (file_id, meta) = if key == "photo" {
                if let Some(arr) = v.as_array() {
                    let largest = arr
                        .iter()
                        .max_by_key(|p| p.get("file_size").and_then(Value::as_i64).unwrap_or(0));
                    match largest {
                        Some(p) => (
                            p.get("file_id").and_then(Value::as_str).map(str::to_string),
                            Some(p.clone()),
                        ),
                        None => (None, None),
                    }
                } else {
                    (None, None)
                }
            } else {
                (
                    v.get("file_id").and_then(Value::as_str).map(str::to_string),
                    Some(v.clone()),
                )
            };
            return (Some(kind.into()), file_id, meta);
        }
    }
    (None, None, None)
}
