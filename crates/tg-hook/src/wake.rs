//! Send the wakeup message and parse `message_id` from the
//! `tg_send_message` response — used as the baseline for reply detection.

use crate::mcp_client::{McpClient, tool_result_text};
use crate::output::DEFAULT_ACK_MESSAGE;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

/// Telegram Bot API hard limit on a message body (4096 characters). We split
/// on byte length, which is conservative for multi-byte text — chunks stay
/// at or under the character limit.
const TELEGRAM_MAX_MESSAGE_BYTES: usize = 4096;

/// Baseline state captured from the wakeup send. Reply detection looks for
/// any *inbound* message with `message_id > sent_message_id`.
#[derive(Debug, Clone, Copy)]
pub struct Baseline {
    /// `message_id` of the message we sent; reply detection starts above this.
    pub sent_message_id: i64,
}

/// Split `text` into chunks of at most `max_len` bytes, breaking at newlines
/// where possible so messages don't cut mid-sentence.
fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;
    while remaining.len() > max_len {
        // Step back to a UTF-8 char boundary at or before max_len.
        let mut boundary = max_len;
        while !remaining.is_char_boundary(boundary) {
            boundary -= 1;
        }
        let slice = &remaining[..boundary];
        // Prefer a newline split so messages break between paragraphs.
        let cut = slice.rfind('\n').unwrap_or(boundary);
        let cut = if cut == 0 { boundary } else { cut };
        chunks.push(remaining[..cut].trim_end().to_string());
        remaining = remaining[cut..].trim_start();
    }
    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }
    chunks
}

/// Send the last assistant response to Telegram, split into chunks within
/// the Bot API message-size limit. Errors on individual chunks are swallowed
/// — we must not block the poll loop on a transient send failure.
pub async fn send_response_chunks(client: &mut McpClient, chat: &str, text: &str) {
    for chunk in split_message(text, TELEGRAM_MAX_MESSAGE_BYTES) {
        let _ = client
            .call_tool("tg_send_message", json!({ "chat": chat, "text": chunk }))
            .await;
    }
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
            json!({ "chat": chat, "text": DEFAULT_ACK_MESSAGE }),
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
    let text = tool_result_text(result).context("tg_send_message result")?;
    let parsed: Value = serde_json::from_str(text).context("decoding tg_send_message text")?;
    let sent_message_id = parsed
        .get("message_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("tg_send_message: missing message_id"))?;
    Ok(Baseline { sent_message_id })
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
        assert_eq!(b.sent_message_id, 7);
    }

    #[test]
    fn missing_content_errors() {
        let result = json!({});
        assert!(extract_baseline(&result).is_err());
    }

    #[test]
    fn split_message_short_text_is_one_chunk() {
        assert_eq!(split_message("hello", 4096), vec!["hello".to_string()]);
    }

    #[test]
    fn split_message_hard_splits_text_without_newlines() {
        let text = "x".repeat(100);
        let chunks = split_message(&text, 30);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.len() <= 30));
        // No whitespace to trim, so the chunks rejoin to the original.
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn split_message_prefers_newline_breaks() {
        let text = format!("{}\n{}", "a".repeat(10), "b".repeat(40));
        let chunks = split_message(&text, 20);
        // First chunk ends at the newline rather than mid-run.
        assert_eq!(chunks[0], "a".repeat(10));
    }

    #[test]
    fn split_message_respects_utf8_boundaries() {
        // 'é' is 2 bytes; 20 of them = 40 bytes, split at 15.
        let text = "é".repeat(20);
        let chunks = split_message(&text, 15);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.len() <= 15));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn split_message_handles_leading_newline() {
        // Newline at offset 0 must not produce a zero-length cut.
        let text = format!("\n{}", "z".repeat(100));
        let chunks = split_message(&text, 30);
        assert!(chunks.iter().all(|c| c.len() <= 30));
    }
}
