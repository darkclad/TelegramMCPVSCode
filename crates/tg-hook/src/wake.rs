//! Send the wakeup message and parse `(chat_id, message_id)` from the
//! `tg_send_message` response — used as the baseline for reply detection.

use crate::mcp_client::McpClient;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

/// Baseline state captured from the wakeup send. Reply detection looks for
/// any *inbound* message in `chat_id` with `message_id > sent_message_id`.
#[derive(Debug, Clone, Copy)]
pub struct Baseline {
    /// Numeric chat identifier returned by the Bot API.
    pub chat_id: i64,
    /// `message_id` of the message we sent; reply detection starts above this.
    pub sent_message_id: i64,
}

/// Send a brief acknowledgement back to the user in Telegram.
///
/// Called right after detecting an inbound reply, before returning the block
/// decision, so the user has immediate feedback that Claude saw their message.
/// Errors are intentionally swallowed — the block decision must go through
/// even if the ack fails.
pub async fn send_ack(client: &mut McpClient, chat: &str) {
    let _ = client
        .call_tool(
            "tg_send_message",
            json!({ "chat": chat, "text": "Got it, working on it..." }),
        )
        .await;
}

/// Call `tg_send_message` with the provided chat + text, return the
/// baseline encoded in the response.
pub async fn send_wakeup(client: &mut McpClient, chat: &str, text: &str) -> Result<Baseline> {
    let result = client
        .call_tool("tg_send_message", json!({ "chat": chat, "text": text }))
        .await?;
    extract_baseline(&result)
}

/// Decode the nested JSON: `result.content[0].text` is a JSON string that
/// parses into `{ chat_id, message_id, date }`.
pub fn extract_baseline(result: &Value) -> Result<Baseline> {
    let text = result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c0| c0.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tg_send_message: missing content[0].text"))?;
    let parsed: Value = serde_json::from_str(text).context("decoding tg_send_message text")?;
    let chat_id = parsed
        .get("chat_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("tg_send_message: missing chat_id"))?;
    let sent_message_id = parsed
        .get("message_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("tg_send_message: missing message_id"))?;
    Ok(Baseline {
        chat_id,
        sent_message_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_baseline_from_nested_payload() {
        let result = json!({
            "content": [
                { "type": "text",
                  "text": "{\"chat_id\": 42, \"message_id\": 7, \"date\": 1700000000}" }
            ],
            "isError": false
        });
        let b = extract_baseline(&result).expect("ok");
        assert_eq!(b.chat_id, 42);
        assert_eq!(b.sent_message_id, 7);
    }

    #[test]
    fn missing_content_errors() {
        let result = json!({});
        assert!(extract_baseline(&result).is_err());
    }
}
