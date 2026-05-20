//! Poll `tg_history_messages` for inbound replies after the baseline.

use crate::mcp_client::{McpClient, tool_result_text};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

/// A reply pulled from history.
#[derive(Debug, Clone)]
pub struct Reply {
    /// `message_id` of the inbound message.
    pub message_id: i64,
    /// Best-effort text. None for media-only messages.
    pub text: Option<String>,
}

/// One poll: return ALL inbound messages after `after_message_id`, sorted
/// oldest-first. Empty vec means no new replies yet.
pub async fn poll_once(
    client: &mut McpClient,
    chat: &str,
    after_message_id: i64,
) -> Result<Vec<Reply>> {
    let result = client
        .call_tool(
            "tg_history_messages",
            json!({
                "chat": chat,
                "after_message_id": after_message_id,
                "limit": 50
            }),
        )
        .await?;
    parse_replies(&result)
}

/// Parse the inbound replies out of a `tg_history_messages` tool result.
///
/// Decodes the MCP text-content envelope, keeps only `direction == "in"`
/// rows, and sorts them oldest-first by `message_id`.
pub fn parse_replies(result: &Value) -> Result<Vec<Reply>> {
    let text = tool_result_text(result).context("tg_history_messages result")?;
    let parsed: Value = serde_json::from_str(text).context("decoding history payload")?;
    let arr = parsed
        .as_array()
        .ok_or_else(|| anyhow!("tg_history_messages: expected JSON array"))?;
    let mut replies: Vec<Reply> = arr
        .iter()
        .filter(|m| m.get("direction").and_then(Value::as_str) == Some("in"))
        .filter_map(|m| {
            let message_id = m.get("message_id").and_then(Value::as_i64)?;
            let text = m.get("text").and_then(Value::as_str).map(str::to_string);
            Some(Reply { message_id, text })
        })
        .collect();
    replies.sort_by_key(|r| r.message_id);
    Ok(replies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Wrap `rows` in the MCP text-content envelope `tg_history_messages`
    /// returns: `content[0].text` is the JSON array as a string.
    fn fake_history_result(rows: &Value) -> Value {
        let text = serde_json::to_string(rows).unwrap();
        json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false
        })
    }

    #[test]
    fn parse_replies_returns_inbound_sorted() {
        // Two inbound + one outbound; both inbound returned, oldest first.
        let result = fake_history_result(&json!([
            { "message_id": 13, "direction": "in",  "text": "third" },
            { "message_id": 12, "direction": "out", "text": "our send" },
            { "message_id": 11, "direction": "in",  "text": "first reply" }
        ]));
        let replies = parse_replies(&result).expect("parses");
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0].message_id, 11);
        assert_eq!(replies[0].text.as_deref(), Some("first reply"));
        assert_eq!(replies[1].message_id, 13);
    }

    #[test]
    fn parse_replies_handles_media_only_and_empty() {
        let result = fake_history_result(&json!([
            { "message_id": 20, "direction": "in" }
        ]));
        let replies = parse_replies(&result).expect("parses");
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].text, None);

        let empty = fake_history_result(&json!([]));
        assert!(parse_replies(&empty).expect("parses").is_empty());
    }

    #[test]
    fn parse_replies_rejects_malformed_envelope() {
        assert!(parse_replies(&json!({})).is_err());
    }
}
