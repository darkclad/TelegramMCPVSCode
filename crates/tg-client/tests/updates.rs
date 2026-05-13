//! Integration tests for [`tg_client::TgClient::get_me`] and
//! [`tg_client::TgClient::get_updates_raw`].
//!
//! Uses `wiremock` to fake `api.telegram.org` so we exercise the wire format
//! end-to-end without touching the network.

use tg_client::TgClient;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn get_updates_returns_serde_json_value() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": [
                {
                    "update_id": 100,
                    "message": {
                        "message_id": 1,
                        "date": 1_700_000_000,
                        "chat": { "id": 42, "type": "private", "first_name": "alice" },
                        "from": { "id": 42, "is_bot": false, "first_name": "alice" },
                        "text": "hello"
                    }
                }
            ]
        })))
        .mount(&server)
        .await;
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let updates = client.get_updates_raw(None, 0, None).await.unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["update_id"].as_i64(), Some(100));
}

#[tokio::test]
async fn get_me_returns_username() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "id": 555, "is_bot": true,
                "first_name": "MyBot", "username": "mybot",
                // teloxide's `Me` deserialiser requires these three flags;
                // they are always present on a real Telegram getMe response
                // but the plan's fixture omits them.
                "can_join_groups": true,
                "can_read_all_group_messages": false,
                "supports_inline_queries": false
            }
        })))
        .mount(&server)
        .await;
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let me = client.get_me().await.unwrap();
    assert_eq!(me.username.as_deref(), Some("mybot"));
}
