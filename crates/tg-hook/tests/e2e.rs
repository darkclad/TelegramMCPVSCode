//! End-to-end smoke for the tg-hook binary.
//!
//! Pipeline:
//!  1. Spawn `TelegramMCP` with a unique `CLAUDE_SESSION_ID` and a
//!     wiremock Bot API stub for `SendMessage`.
//!  2. Wait for the MCP server's discovery file to appear.
//!  3. Seed an inbound history row matching the alias `test`.
//!  4. Spawn the `tg-hook` binary with `--chat test --message wake`
//!     and the same `CLAUDE_SESSION_ID`.
//!  5. Read its stdout; assert it emits a `decision: block` with the
//!     seeded reply text.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests panic on infra failures"
)]

mod common;

use common::{ServerGuard, make_config, spawn_server, tg_hook_binary};
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn hook_returns_decision_block_with_inbound_reply() {
    let session_id = format!("tg-hook-test-{}", uuid_v4_simple());

    // ---- Bot API stub ----
    let bot = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 100, "date": 1_700_000_000,
                "chat": { "id": 42, "type": "private" }
            }
        })))
        .mount(&bot)
        .await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    std::fs::write(
        &cfg_path,
        make_config(&bot.uri(), &db_path, 42, &session_id),
    )
    .unwrap();

    // ---- Spawn TelegramMCP (drives MCP initialize on stdio internally) ----
    let _server: ServerGuard = tokio::task::spawn_blocking({
        let cfg_path = cfg_path.clone();
        let session_id = session_id.clone();
        move || spawn_server(&cfg_path, &session_id)
    })
    .await
    .unwrap();

    // Wait for the discovery file with this session_id to appear (the
    // local-pipe server writes it at startup).
    let discovery_dir = local_pipe::discovery::discovery_dir().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() <= deadline,
            "TelegramMCP did not publish a discovery record in time"
        );
        let mut found = false;
        if let Ok(entries) = std::fs::read_dir(&discovery_dir) {
            for entry in entries.flatten() {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    if let Ok(rec) = serde_json::from_slice::<local_pipe::DiscoveryRecord>(&bytes) {
                        if rec.session_id.as_deref() == Some(&session_id) {
                            found = true;
                            break;
                        }
                    }
                }
            }
        }
        if found {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // ---- Seed an inbound reply directly into the SQLite store ----
    // The hook expects an inbound row with message_id > 100 (the
    // wakeup's sent_message_id from the stubbed SendMessage response).
    seed_inbound(&db_path, 42, 101, "I am the reply").await;

    // ---- Run the hook ----
    let hook_out = tokio::task::spawn_blocking({
        let session_id = session_id.clone();
        move || {
            Command::new(tg_hook_binary())
                .args([
                    "--chat",
                    "test",
                    "--message",
                    "wake",
                    "--poll-secs",
                    "1",
                    "--timeout-secs",
                    "10",
                ])
                .env("CLAUDE_SESSION_ID", &session_id)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn tg-hook")
                .wait_with_output_after_stdin("{}\n")
                .expect("hook output")
        }
    })
    .await
    .unwrap();

    assert!(
        hook_out.status.success(),
        "tg-hook exited non-zero: status={:?}\nstderr=\n{}",
        hook_out.status,
        String::from_utf8_lossy(&hook_out.stderr),
    );
    let stdout = String::from_utf8(hook_out.stdout).unwrap();
    let last = stdout.lines().last().expect("stdout has a JSON line");
    let parsed: serde_json::Value = serde_json::from_str(last).expect("stdout is JSON");
    assert_eq!(parsed["decision"], "block");
    let reason = parsed["reason"].as_str().expect("reason is string");
    assert!(reason.contains("I am the reply"), "reason was: {reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn hook_emits_retry_message_on_timeout() {
    let session_id = format!("tg-hook-test-{}", uuid_v4_simple());

    let bot = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/bot12345:fake/SendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 100, "date": 1_700_000_000,
                "chat": { "id": 42, "type": "private" }
            }
        })))
        .mount(&bot)
        .await;

    let dir = tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    let db_path = dir.path().join("h.db");
    std::fs::write(
        &cfg_path,
        make_config(&bot.uri(), &db_path, 42, &session_id),
    )
    .unwrap();

    let _server: ServerGuard = tokio::task::spawn_blocking({
        let cfg_path = cfg_path.clone();
        let session_id = session_id.clone();
        move || spawn_server(&cfg_path, &session_id)
    })
    .await
    .unwrap();

    let discovery_dir = local_pipe::discovery::discovery_dir().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() <= deadline,
            "TelegramMCP did not publish a discovery record in time"
        );
        let mut found = false;
        if let Ok(entries) = std::fs::read_dir(&discovery_dir) {
            for entry in entries.flatten() {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    if let Ok(rec) = serde_json::from_slice::<local_pipe::DiscoveryRecord>(&bytes) {
                        if rec.session_id.as_deref() == Some(&session_id) {
                            found = true;
                            break;
                        }
                    }
                }
            }
        }
        if found {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // No seed_inbound here: the hook should time out and emit the retry-message.
    let hook_out = tokio::task::spawn_blocking({
        let session_id = session_id.clone();
        move || {
            Command::new(tg_hook_binary())
                .args([
                    "--chat",
                    "test",
                    "--message",
                    "wake",
                    "--retry-message",
                    "RETRY-OK",
                    "--poll-secs",
                    "1",
                    "--timeout-secs",
                    "2",
                ])
                .env("CLAUDE_SESSION_ID", &session_id)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn tg-hook")
                .wait_with_output_after_stdin("{}\n")
                .expect("hook output")
        }
    })
    .await
    .unwrap();

    // db_path is not used in this test (no seed), suppress the unused binding
    // warning that would arise from moving it out of the let-else.
    let _ = db_path;

    assert!(
        hook_out.status.success(),
        "hook failed: {:?}\nstderr=\n{}",
        hook_out.status,
        String::from_utf8_lossy(&hook_out.stderr),
    );
    let stdout = String::from_utf8(hook_out.stdout).unwrap();
    let last = stdout.lines().last().expect("stdout has a JSON line");
    let parsed: serde_json::Value = serde_json::from_str(last).expect("stdout is JSON");
    assert_eq!(parsed["decision"], "block");
    assert_eq!(parsed["reason"], "RETRY-OK");
}

/// Helper: insert an inbound message row by opening the `SQLite` db directly.
/// We deliberately bypass MCP — directly seeding is the cleanest way to
/// simulate "user replied" without standing up a fake long-poll server.
async fn seed_inbound(db_path: &std::path::Path, chat_id: i64, message_id: i64, text: &str) {
    let db_path = db_path.to_owned();
    let text = text.to_owned();
    tokio::task::spawn_blocking(move || {
        let store = history::History::open(&db_path).expect("open history");
        // History uses async methods backed by spawn_blocking internally;
        // drive them with a fresh single-threaded runtime isolated from the
        // outer multi-thread test runtime so block_on doesn't panic.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                store
                    .upsert_chat(&history::ChatInfo {
                        chat_id,
                        kind: history::ChatKind::Private,
                        title: None,
                        username: None,
                        first_seen: 1_700_000_000,
                        last_seen: 1_700_000_000,
                    })
                    .await
                    .unwrap();
                store
                    .insert_message(&history::StoredMessage {
                        chat_id,
                        message_id,
                        date: 1_700_000_100,
                        from_id: Some(42),
                        from_name: Some("Tester".into()),
                        reply_to: None,
                        text: Some(text),
                        media_kind: None,
                        media_file_id: None,
                        media_meta: None,
                        direction: history::Direction::In,
                        raw: serde_json::json!({"seeded": true}),
                    })
                    .await
                    .unwrap();
            });
    })
    .await
    .unwrap();
}

/// Generate a simple unique id without pulling in the `uuid` crate as a
/// dev-dep here; reuse the workspace dep.
fn uuid_v4_simple() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Tiny shim trait so we can write `wait_with_output_after_stdin` cleanly.
trait ChildExt {
    fn wait_with_output_after_stdin(
        self,
        stdin_payload: &str,
    ) -> std::io::Result<std::process::Output>;
}
impl ChildExt for std::process::Child {
    fn wait_with_output_after_stdin(
        mut self,
        stdin_payload: &str,
    ) -> std::io::Result<std::process::Output> {
        if let Some(mut stdin) = self.stdin.take() {
            stdin.write_all(stdin_payload.as_bytes())?;
            stdin.flush()?;
            drop(stdin);
        }
        self.wait_with_output()
    }
}
