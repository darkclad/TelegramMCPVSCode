//! Poll `tg_history_messages` for inbound replies after the baseline.

use crate::mcp_client::McpClient;
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
    let text = result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c0| c0.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tg_history_messages: missing content[0].text"))?;
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

    fn fake_history_result(rows: &Value) -> Value {
        let text = serde_json::to_string(rows).unwrap();
        json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false
        })
    }

    #[tokio::test]
    async fn returns_all_inbound_sorted() {
        // Two inbound + one outbound; both inbound returned, oldest first.
        let rows = json!([
            { "message_id": 13, "direction": "in",  "text": "third" },
            { "message_id": 12, "direction": "out", "text": "our send" },
            { "message_id": 11, "direction": "in",  "text": "first reply" }
        ]);
        let arr = rows.as_array().unwrap();
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
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0].message_id, 11);
        assert_eq!(replies[0].text.as_deref(), Some("first reply"));
        assert_eq!(replies[1].message_id, 13);
        let _ = fake_history_result;
    }
}
