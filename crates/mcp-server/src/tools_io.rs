//! Tool input/output types.
//!
//! Every type derives `serde::Deserialize` (or `Serialize` for outputs) plus
//! `schemars::JsonSchema` so the input schema can be generated automatically
//! by the `schema_obj` helper in `main.rs`. Task 18 wires `BotWhoamiInput`
//! and `ListAliasesInput`; the remaining types land in subsequent tool
//! tasks.

#![allow(dead_code, reason = "remaining tools land in Tasks 19-21")]

use aliases::ChatRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Input for the `send_message` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendMessageInput {
    /// Target chat reference (alias name or numeric id).
    pub chat: ChatRef,
    /// Message body. The Bot API caps this at 4096 chars.
    pub text: String,
    /// Optional Telegram parse mode (`MarkdownV2`, `HTML`, ...).
    #[serde(default)]
    pub parse_mode: Option<String>,
    /// Optional message id to reply to.
    #[serde(default)]
    pub reply_to: Option<i64>,
    /// When `Some(true)`, sends without a notification sound.
    #[serde(default)]
    pub silent: Option<bool>,
    /// When `Some(false)`, disables the URL link preview.
    #[serde(default)]
    pub link_preview: Option<bool>,
}

/// Input for the `send_photo` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendPhotoInput {
    /// Target chat reference.
    pub chat: ChatRef,
    /// Local filesystem path of the image to upload.
    pub path: PathBuf,
    /// Optional caption text shown under the photo.
    #[serde(default)]
    pub caption: Option<String>,
    /// Optional Telegram parse mode for the caption.
    #[serde(default)]
    pub parse_mode: Option<String>,
}

/// Input for the `send_document` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendDocumentInput {
    /// Target chat reference.
    pub chat: ChatRef,
    /// Local filesystem path of the document to upload.
    pub path: PathBuf,
    /// Optional caption text shown alongside the document.
    #[serde(default)]
    pub caption: Option<String>,
    /// Optional override for the filename Telegram displays.
    #[serde(default)]
    pub filename: Option<String>,
}

/// Input for the `edit_message` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditMessageInput {
    /// Chat containing the message to edit.
    pub chat: ChatRef,
    /// Identifier of the message to edit.
    pub message_id: i64,
    /// Replacement text.
    pub text: String,
    /// Optional Telegram parse mode for the new text.
    #[serde(default)]
    pub parse_mode: Option<String>,
}

/// Input for the `delete_message` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteMessageInput {
    /// Chat containing the message to delete.
    pub chat: ChatRef,
    /// Identifier of the message to delete.
    pub message_id: i64,
}

/// Input for the `forward_message` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForwardMessageInput {
    /// Source chat of the original message.
    pub from_chat: ChatRef,
    /// Identifier of the message in `from_chat`.
    pub message_id: i64,
    /// Destination chat for the forwarded copy.
    pub to_chat: ChatRef,
}

/// Input for the `send_chat_action` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChatActionInput {
    /// Chat that should display the typing/upload indicator.
    pub chat: ChatRef,
    /// Bot API chat-action string (e.g. `typing`, `upload_photo`).
    pub action: String,
}

/// Output of the `send_message` / `edit_message` family of tools.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SendMessageOutput {
    /// Numeric chat id of the message that was sent or edited.
    pub chat_id: i64,
    /// Identifier of the resulting message inside `chat_id`.
    pub message_id: i64,
    /// Telegram-side timestamp in unix seconds.
    pub date: i64,
}

/// Input for the `list_chats` tool. Currently takes no arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListChatsInput {}

/// Input for the `get_chat` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetChatInput {
    /// Chat to look up.
    pub chat: ChatRef,
}

/// Input for the `history_messages` tool: paginated history reads.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistoryMessagesInput {
    /// Chat whose history to read.
    pub chat: ChatRef,
    /// Return messages strictly older than this id.
    #[serde(default)]
    pub before_message_id: Option<i64>,
    /// Return messages strictly newer than this id.
    #[serde(default)]
    pub after_message_id: Option<i64>,
    /// Maximum number of messages to return. Defaults to 50.
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    50
}

/// Input for the `history_search` tool: FTS5 search over stored messages.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistorySearchInput {
    /// Search query in FTS5 syntax.
    pub query: String,
    /// Restrict results to a single chat.
    #[serde(default)]
    pub chat: Option<ChatRef>,
    /// Lower bound on message date, unix seconds inclusive.
    #[serde(default)]
    pub since: Option<i64>,
    /// Upper bound on message date, unix seconds inclusive.
    #[serde(default)]
    pub until: Option<i64>,
}

/// Input for the `get_message` tool: fetch a single stored message.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMessageInput {
    /// Chat containing the message.
    pub chat: ChatRef,
    /// Identifier of the message inside `chat`.
    pub message_id: i64,
}

/// Input for the `mark_read` tool: mark messages up to an id as read.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkReadInput {
    /// Chat to mark as read.
    pub chat: ChatRef,
    /// Mark messages with id `<= up_to_message_id` as read.
    pub up_to_message_id: i64,
}

/// Input for the `download` tool: download an attachment from a message.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DownloadInput {
    /// Chat containing the message with the attachment.
    pub chat: ChatRef,
    /// Identifier of the message whose attachment to download.
    pub message_id: i64,
    /// Filesystem path the bytes should be written to.
    pub dest_path: PathBuf,
}

/// Input for the `bot_whoami` tool. Takes no arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BotWhoamiInput {}

/// Input for the `list_aliases` tool. Takes no arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAliasesInput {}
