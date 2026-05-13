//! Integration tests for [`tg_updater::Updater::run`] polling loop.

use history::History;
use serde_json::json;
use std::time::Duration;
use tempfile::tempdir;
use tg_client::TgClient;
use tg_updater::{Updater, UpdaterConfig};
use url::Url;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn updater_consumes_batch_persists_offset_and_messages() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();

    // first call returns 1 update
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetUpdates"))
        .and(body_partial_json(json!({ "offset": 0 })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": [
                {
                    "update_id": 100,
                    "message": {
                        "message_id": 1, "date": 1_700_000_000,
                        "chat": { "id": 42, "type": "private", "first_name": "alice" },
                        "from": { "id": 42, "is_bot": false, "first_name": "alice" },
                        "text": "hello"
                    }
                }
            ]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // subsequent calls return empty
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "result": []
        })))
        .mount(&server)
        .await;

    let dir = tempdir().unwrap();
    let store = History::open(dir.path().join("h.db")).unwrap();
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let config = UpdaterConfig {
        poll_timeout_secs: 1,
        allowed_update_kinds: vec!["message".into()],
        allowed_chats: None,
    };
    let updater = Updater { client, store: store.clone(), config };

    let handle = tokio::spawn(updater.run());
    // give it time to process one batch
    tokio::time::sleep(Duration::from_millis(400)).await;
    handle.abort();

    // message landed
    let msg = store.get_message(42, 1).await.unwrap();
    assert_eq!(msg.text.as_deref(), Some("hello"));
    // offset persisted as 101 (last_update_id + 1)
    assert_eq!(store.kv_get("update_offset").await.unwrap().as_deref(), Some("101"));
}

#[tokio::test]
async fn updater_drops_updates_from_disallowed_chats() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetUpdates"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "result": [
                {
                    "update_id": 1,
                    "message": {
                        "message_id": 1, "date": 1,
                        "chat": { "id": 999, "type": "private", "first_name": "stranger" },
                        "from": { "id": 999, "is_bot": false, "first_name": "stranger" },
                        "text": "spam"
                    }
                }
            ]
        })))
        .mount(&server)
        .await;
    let dir = tempdir().unwrap();
    let store = History::open(dir.path().join("h.db")).unwrap();
    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let config = UpdaterConfig {
        poll_timeout_secs: 1,
        allowed_update_kinds: vec!["message".into()],
        allowed_chats: Some(vec![42]),
    };
    let updater = Updater { client, store: store.clone(), config };
    let handle = tokio::spawn(updater.run());
    tokio::time::sleep(Duration::from_millis(300)).await;
    handle.abort();

    // chat 999 was filtered → no chat row, no message row
    let chats = store.list_chats().await.unwrap();
    assert!(chats.is_empty());
}
