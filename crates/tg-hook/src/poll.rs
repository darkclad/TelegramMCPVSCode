//! Poll `tg_history_messages` for inbound replies after the baseline.

use crate::mcp_client::McpClient;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

/// A reply pulled from history.
#[derive(Debug, Clone)]
pub struct Reply {
    /// `message_id` of the inbound message.
    pub message_id: i64,
    /// Best-effort text. None for media-only messages — caller decides
    /// whether to surface "(media)" to Claude or keep waiting.
    pub text: Option<String>,
}

/// One poll: ask the server for messages after `after_message_id` in
/// `chat`, return the OLDEST inbound message in the response, if any.
///
/// `chat` is whatever the user passed on the CLI (alias or numeric id) —
/// the MCP server re-resolves on every call, which is fine for our
/// once-every-5-seconds rate.
pub async fn poll_once(
    client: &mut McpClient,
    chat: &str,
    after_message_id: i64,
) -> Result<Option<Reply>> {
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
    let oldest_in: Option<&Value> = arr
        .iter()
        .filter(|m| m.get("direction").and_then(Value::as_str) == Some("in"))
        .min_by_key(|m| {
            m.get("message_id")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX)
        });
    let Some(m) = oldest_in else {
        return Ok(None);
    };
    let message_id = m
        .get("message_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("history row missing message_id"))?;
    let message_text = m.get("text").and_then(Value::as_str).map(str::to_string);
    Ok(Some(Reply {
        message_id,
        text: message_text,
    }))
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
    async fn picks_oldest_inbound_only() {
        // Two inbound messages + one outbound; we should get message_id=11.
        let rows = json!([
            { "message_id": 13, "direction": "in",  "text": "third" },
            { "message_id": 12, "direction": "out", "text": "our send" },
            { "message_id": 11, "direction": "in",  "text": "first reply" }
        ]);
        let parsed = rows;
        let oldest_in = parsed
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m.get("direction").and_then(Value::as_str) == Some("in"))
            .min_by_key(|m| {
                m.get("message_id")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX)
            });
        let m = oldest_in.unwrap();
        assert_eq!(m["message_id"], 11);
        assert_eq!(m["text"], "first reply");
        let _ = fake_history_result;
    }
}
