//! End-to-end smoke tests for the `TelegramMCP` binary.
//!
//! Each test spawns the real compiled binary as a child process, points it
//! at a `wiremock`-backed fake Bot API and a `tempdir` history database, and
//! drives the MCP handshake + tool calls over stdio.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests panic on infra failures"
)]

mod common;

use common::{McpClient, binary_path};
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Render a minimal config TOML pointing at the given fake Bot API and
/// history db location, with a single `test = <alias_id>` alias.
fn make_config(api_base: &str, db: &std::path::Path, alias_id: i64) -> String {
    let db = db.display().to_string().replace('\\', "/");
    format!(
        r#"
[bot]
token = "12345:fake"
api_base_url = "{api_base}"

[storage]
path = "{db}"

[updater]
enabled = false

[aliases]
test = {alias_id}
"#
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "id": 555, "is_bot": true,
                "first_name": "TestBot", "username": "testbot",
                "can_join_groups": false,
                "can_read_all_group_messages": false,
                "supports_inline_queries": false
            }
        })))
        .mount(&server)
        .await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    std::fs::write(&cfg_path, make_config(&server.uri(), &db_path, 42)).unwrap();

    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    let resp = client.call_tool("tg_bot_whoami", serde_json::json!({}));
    let content = &resp["result"]["content"][0]["text"];
    let parsed: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(parsed["username"], "testbot");
}

#[tokio::test(flavor = "multi_thread")]
async fn send_message_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 7, "date": 1_700_000_000,
                "chat": { "id": 42, "type": "private" }
            }
        })))
        .mount(&server)
        .await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    std::fs::write(&cfg_path, make_config(&server.uri(), &db_path, 42)).unwrap();

    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    let resp = client.call_tool(
        "tg_send_message",
        serde_json::json!({
            "chat": "test", "text": "hello"
        }),
    );
    let content = &resp["result"]["content"][0]["text"];
    let parsed: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(parsed["message_id"], 7);
    assert_eq!(parsed["chat_id"], 42);
}

#[tokio::test(flavor = "multi_thread")]
async fn send_then_history_messages_readback() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 1, "date": 1_700_000_000,
                "chat": { "id": 42, "type": "private" }
            }
        })))
        .mount(&server)
        .await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    std::fs::write(&cfg_path, make_config(&server.uri(), &db_path, 42)).unwrap();
    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    let _ = client.call_tool(
        "tg_send_message",
        serde_json::json!({
            "chat": "test", "text": "stored"
        }),
    );
    let resp = client.call_tool(
        "tg_history_messages",
        serde_json::json!({
            "chat": "test", "limit": 10
        }),
    );
    let content = &resp["result"]["content"][0]["text"];
    let parsed: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(parsed[0]["text"], "stored");
    assert_eq!(parsed[0]["direction"], "out");
}

#[tokio::test(flavor = "multi_thread")]
async fn send_to_disallowed_chat_returns_error() {
    let server = MockServer::start().await;
    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    let api_base = server.uri();
    let db = db_path.display().to_string().replace('\\', "/");
    let cfg = format!(
        r#"
[bot]
token = "12345:fake"
api_base_url = "{api_base}"

[storage]
path = "{db}"

[updater]
enabled = false

[aliases]
allowed = 100
stranger = 200

[access]
allowed_send_targets = ["allowed"]
"#
    );
    std::fs::write(&cfg_path, cfg).unwrap();

    let mut client = McpClient::spawn(&binary_path(), &cfg_path);
    client.initialize();
    let resp = client.call_tool(
        "tg_send_message",
        serde_json::json!({
            "chat": "stranger", "text": "blocked"
        }),
    );
    assert!(
        resp.get("error").is_some(),
        "expected an error response, got {resp}"
    );
}
