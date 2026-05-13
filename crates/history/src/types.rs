//! Public value types for the [`history`](crate) store.

use serde::{Deserialize, Serialize};

/// Direction of a stored message relative to the local account.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Message received from the peer.
    In,
    /// Message sent by the local account.
    Out,
}

impl Direction {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
    pub(crate) fn from_sql(s: &str) -> Option<Self> {
        match s {
            "in" => Some(Direction::In),
            "out" => Some(Direction::Out),
            _ => None,
        }
    }
}

/// Kind of Telegram chat a stored message belongs to.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatKind {
    /// One-to-one private chat.
    Private,
    /// Basic (legacy) group chat.
    Group,
    /// Supergroup.
    Supergroup,
    /// Broadcast channel.
    Channel,
}

impl ChatKind {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            ChatKind::Private => "private",
            ChatKind::Group => "group",
            ChatKind::Supergroup => "supergroup",
            ChatKind::Channel => "channel",
        }
    }
    pub(crate) fn from_sql(s: &str) -> Option<Self> {
        match s {
            "private" => Some(ChatKind::Private),
            "group" => Some(ChatKind::Group),
            "supergroup" => Some(ChatKind::Supergroup),
            "channel" => Some(ChatKind::Channel),
            _ => None,
        }
    }
}

/// Static information about a chat recorded in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatInfo {
    /// Telegram chat id.
    pub chat_id: i64,
    /// Kind of chat (private, group, supergroup, channel).
    pub kind: ChatKind,
    /// Human-readable chat title, when known.
    pub title: Option<String>,
    /// Public `@username` for the chat, when known.
    pub username: Option<String>,
    /// Unix timestamp when the chat was first seen by this store.
    pub first_seen: i64,
    /// Unix timestamp when the chat was most recently seen by this store.
    pub last_seen: i64,
}

/// A chat plus lightweight aggregate state for listings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSummary {
    /// Underlying chat information.
    #[serde(flatten)]
    pub info: ChatInfo,
    /// Number of unread messages currently recorded for the chat.
    pub unread_count: i64,
    /// Message id of the most recent stored message, if any.
    pub last_message_id: Option<i64>,
}

/// A single message as persisted in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    /// Telegram chat id the message belongs to.
    pub chat_id: i64,
    /// Telegram message id within the chat.
    pub message_id: i64,
    /// Unix timestamp of the message.
    pub date: i64,
    /// Sender's Telegram user id, when known.
    pub from_id: Option<i64>,
    /// Sender's display name at the time of storage.
    pub from_name: Option<String>,
    /// Message id this message replies to, if any.
    pub reply_to: Option<i64>,
    /// Text content of the message, when present.
    pub text: Option<String>,
    /// Coarse media kind (e.g. `photo`, `document`), when the message carries media.
    pub media_kind: Option<String>,
    /// Telegram file id for the primary media attachment, when applicable.
    pub media_file_id: Option<String>,
    /// Additional structured metadata about the media.
    pub media_meta: Option<serde_json::Value>,
    /// Direction of the message relative to the local account.
    pub direction: Direction,
    /// Raw upstream payload, preserved for forward compatibility.
    pub raw: serde_json::Value,
}

/// A single hit from a full-text search query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    /// Chat id of the matched message.
    pub chat_id: i64,
    /// Message id of the matched message.
    pub message_id: i64,
    /// Unix timestamp of the matched message.
    pub date: i64,
    /// Snippet of text around the match, suitable for display.
    pub snippet: String,
}
