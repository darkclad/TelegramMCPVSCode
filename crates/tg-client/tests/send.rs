//! Integration tests for [`tg_client::TgClient::send_message`].
//!
//! Uses `wiremock` to fake `api.telegram.org` so we can assert wire-format
//! interactions and error mapping without touching the network.

use tg_client::TgClient;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn send_message_posts_and_parses_response() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();

    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 7,
                "date": 1_700_000_000_i64,
                "chat": { "id": 42, "type": "private" },
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let sent = client
        .send_message(42, "hi", None, None, false, true)
        .await
        .unwrap();
    assert_eq!(sent.chat_id, 42);
    assert_eq!(sent.message_id, 7);
    assert_eq!(sent.date, 1_700_000_000);
}

#[tokio::test]
async fn rate_limit_maps_to_rate_limited_error() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();

    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "ok": false,
            "error_code": 429,
            "description": "Too Many Requests: retry after 7",
            "parameters": { "retry_after": 7 }
        })))
        .mount(&server)
        .await;

    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let err = client
        .send_message(42, "hi", None, None, false, true)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        tg_client::TgClientError::RateLimited {
            retry_after_secs: 7
        }
    ));
}
