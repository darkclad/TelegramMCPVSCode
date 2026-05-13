//! Integration tests for [`tg_client::TgClient::download_file`].
//!
//! Uses `wiremock` to fake both `getFile` (JSON metadata) and the
//! `/file/bot<token>/...` byte-stream endpoint so we can assert that the
//! two-step Bot API download flow writes the expected bytes to disk.

use tempfile::tempdir;
use tg_client::TgClient;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn download_writes_bytes_to_dest_path() {
    let server = MockServer::start().await;
    let api_base = Url::parse(&server.uri()).unwrap();

    Mock::given(method("POST"))
        .and(path("/bot12345:fake/GetFile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": { "file_id": "abc", "file_unique_id": "abc-u", "file_path": "documents/file_123.bin", "file_size": 5 }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/file/bot12345:fake/documents/file_123.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello"))
        .mount(&server)
        .await;

    let client = TgClient::new("12345:fake".into(), Some(api_base)).unwrap();
    let dir = tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let bytes_written = client.download_file("abc", &dest).await.unwrap();
    assert_eq!(bytes_written, 5);
    assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
}
