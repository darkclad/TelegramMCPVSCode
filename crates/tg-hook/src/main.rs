//! Claude Code hook: bridges a session to a Telegram chat.
//!
//! Dispatches on the hook event in stdin:
//! - `Stop` — send a wakeup + the last response to Telegram, block polling
//!   for the user's reply, return it to Claude as the next turn.
//! - `PreToolUse` (`AskUserQuestion`) — relay the question to Telegram and
//!   block the tool with the user's reply.
//!
//! See module docs in `lib.rs` for the per-stage detail.

#![cfg(windows)]

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::time::Duration;
use tg_hook::cli::CliArgs;
use tg_hook::discovery::{load_all, pick_record, pid_chain};
use tg_hook::local_input::local_user_active;
use tg_hook::mcp_client::McpClient;
use tg_hook::output::{
    DEFAULT_RETRY_MESSAGE, DEFAULT_WAKEUP_MESSAGE, emit_block, emit_status, emit_tool_block,
};
use tg_hook::poll::poll_once;
use tg_hook::question::QuestionSet;
use tg_hook::stop_input::StopInput;
use tg_hook::wake::{send_ack, send_response_chunks, send_wakeup};

/// Poll interval for the local-input watcher — fast enough to release the
/// hook promptly once the user starts typing into the Claude Code window.
const LOCAL_INPUT_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() {
    // We deliberately swallow errors at the top level: any failure becomes
    // a non-blocking exit so Claude can continue normally. The user sees
    // what went wrong via stderr (Claude Code surfaces hook stderr).
    if let Err(e) = run().await {
        emit_status(&format!("tg-hook error: {e:#}"));
        std::process::exit(0);
    }
}

/// Read the hook's stdin as JSON. Empty or malformed stdin yields an empty
/// object, so the hook can also be smoke-tested by hand from a terminal.
fn read_stdin_json() -> Value {
    use std::io::Read as _;
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        return Value::Object(serde_json::Map::new());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

/// Locate this session's `TelegramMCP` server and open an authenticated
/// pipe connection to it. Shared by both hook flows.
async fn connect_to_server(session_id_env: Option<&str>, pid_chain: &[u32]) -> Result<McpClient> {
    let records = load_all().context("loading discovery records")?;
    if records.is_empty() {
        return Err(anyhow!(
            "no TelegramMCP discovery records found — is the MCP server running?"
        ));
    }
    let record = pick_record(&records, session_id_env, pid_chain)
        .ok_or_else(|| {
            anyhow!(
                "no TelegramMCP discovery record matched (session_id={session_id_env:?}, \
                 pid_chain={pid_chain:?})"
            )
        })?
        .clone();
    McpClient::connect(&record.pipe, &record.token, record.pid)
        .await
        .context("connecting to TelegramMCP pipe")
}

/// Dispatch on the hook event: `PreToolUse` (`AskUserQuestion`) vs. `Stop`.
async fn run() -> Result<()> {
    let cli = CliArgs::parse_env()?;
    let input = read_stdin_json();
    let chain = pid_chain();
    match input.get("hook_event_name").and_then(Value::as_str) {
        Some("PreToolUse") => run_ask(&cli, &input, &chain).await,
        _ => run_stop(&cli, &input, &chain).await,
    }
}

/// `Stop` hook: wake the user on Telegram with the last response, then block
/// until they reply, Ctrl+C, local input, or the timeout fires.
#[allow(
    clippy::cognitive_complexity,
    reason = "linear orchestration; splitting hurts readability"
)]
async fn run_stop(cli: &CliArgs, input: &Value, chain: &[u32]) -> Result<()> {
    let stop_input = StopInput::from_value(input).context("parsing Stop hook stdin")?;

    let local_threshold_ms: u32 =
        u32::try_from(cli.local_input_threshold_secs.saturating_mul(1000)).unwrap_or(u32::MAX);

    // Early exit if the user is already at the keyboard — skip the Telegram
    // wakeup so we don't spam Telegram on every turn while they watch.
    if cli.release_on_local_input && local_user_active(local_threshold_ms, chain) {
        emit_status("tg-hook: local input detected at startup, skipping Telegram wakeup");
        return Ok(());
    }

    let session_id_env = stop_input
        .session_id
        .clone()
        .or_else(|| std::env::var("CLAUDE_SESSION_ID").ok());
    let mut client = connect_to_server(session_id_env.as_deref(), chain).await?;

    // Send the wakeup; the sent message is the baseline for reply detection.
    let message = cli.message.as_deref().unwrap_or(DEFAULT_WAKEUP_MESSAGE);
    let baseline = send_wakeup(&mut client, &cli.chat, message)
        .await
        .context("sending wakeup")?;

    // Follow it with the last assistant response, split into chunks.
    if let Some(text) = &stop_input.last_assistant_message {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            send_response_chunks(&mut client, &cli.chat, trimmed).await;
        }
    }

    emit_status("Waiting on keyboard interrupt or external message");

    let timeout = tokio::time::sleep(Duration::from_secs(cli.timeout_secs));
    tokio::pin!(timeout);
    let mut interval = tokio::time::interval(Duration::from_secs(cli.poll_secs));
    interval.tick().await;
    let mut local_interval = tokio::time::interval(LOCAL_INPUT_POLL_INTERVAL);
    local_interval.tick().await;

    loop {
        tokio::select! {
            biased; // ctrl-c first — never let a poll mask a user-driven exit
            _ = tokio::signal::ctrl_c() => {
                emit_status("tg-hook: interrupted, releasing Claude");
                return Ok(());
            }
            () = &mut timeout => {
                let msg = cli.retry_message.as_deref().unwrap_or(DEFAULT_RETRY_MESSAGE);
                emit_block(msg);
                return Ok(());
            }
            _ = local_interval.tick(), if cli.release_on_local_input => {
                if local_user_active(local_threshold_ms, chain) {
                    emit_status("tg-hook: local input detected, releasing Claude");
                    return Ok(());
                }
            }
            _ = interval.tick() => {
                match poll_once(&mut client, &cli.chat, baseline.sent_message_id).await {
                    Ok(replies) if !replies.is_empty() => {
                        send_ack(&mut client, &cli.chat).await;
                        let reason = if replies.len() == 1 {
                            replies[0]
                                .text
                                .clone()
                                .unwrap_or_else(|| "(media-only message)".into())
                        } else {
                            replies
                                .iter()
                                .enumerate()
                                .map(|(i, r)| {
                                    let t = r.text.as_deref().unwrap_or("(media-only message)");
                                    format!("[{}] {t}", i + 1)
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        };
                        emit_block(&reason);
                        return Ok(());
                    }
                    Ok(_) => {}
                    Err(e) => {
                        emit_status(&format!("tg-hook: poll failed (will retry): {e:#}"));
                    }
                }
            }
        }
    }
}

/// `PreToolUse` hook for `AskUserQuestion`: relay the question to Telegram
/// and block the tool with the user's reply.
///
/// Any failure, Ctrl+C, local input, or timeout returns `Ok(())` — the tool
/// is then allowed and the in-app dialog runs normally. The tool is blocked
/// (with the answer) only on a real Telegram reply.
#[allow(
    clippy::cognitive_complexity,
    reason = "linear orchestration; splitting hurts readability"
)]
async fn run_ask(cli: &CliArgs, input: &Value, chain: &[u32]) -> Result<()> {
    // The hook matcher should scope this to AskUserQuestion; if something
    // else slips through, allow it (return Ok) rather than block it.
    if input.get("tool_name").and_then(Value::as_str) != Some("AskUserQuestion") {
        return Ok(());
    }
    let tool_input = input
        .get("tool_input")
        .ok_or_else(|| anyhow!("PreToolUse payload missing tool_input"))?;
    let questions = QuestionSet::from_tool_input(tool_input)?;

    let local_threshold_ms: u32 =
        u32::try_from(cli.local_input_threshold_secs.saturating_mul(1000)).unwrap_or(u32::MAX);

    // User already at the keyboard → let the in-app dialog handle it.
    if cli.release_on_local_input && local_user_active(local_threshold_ms, chain) {
        emit_status("tg-hook: local input at startup — AskUserQuestion stays in-app");
        return Ok(());
    }

    let session_id_env = input
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| std::env::var("CLAUDE_SESSION_ID").ok());
    let mut client = connect_to_server(session_id_env.as_deref(), chain).await?;

    // Send the rendered question; the sent message is the reply baseline.
    let baseline = send_wakeup(&mut client, &cli.chat, &questions.render())
        .await
        .context("sending question to Telegram")?;

    emit_status("Question sent to Telegram — waiting for your reply");

    let timeout = tokio::time::sleep(Duration::from_secs(cli.timeout_secs));
    tokio::pin!(timeout);
    let mut interval = tokio::time::interval(Duration::from_secs(cli.poll_secs));
    interval.tick().await;
    let mut local_interval = tokio::time::interval(LOCAL_INPUT_POLL_INTERVAL);
    local_interval.tick().await;

    loop {
        tokio::select! {
            biased; // ctrl-c first — never let a poll mask a user-driven exit
            _ = tokio::signal::ctrl_c() => {
                emit_status("tg-hook: interrupted — AskUserQuestion stays in-app");
                return Ok(());
            }
            () = &mut timeout => {
                emit_status("tg-hook: no Telegram reply — AskUserQuestion stays in-app");
                return Ok(());
            }
            _ = local_interval.tick(), if cli.release_on_local_input => {
                if local_user_active(local_threshold_ms, chain) {
                    emit_status("tg-hook: local input — AskUserQuestion stays in-app");
                    return Ok(());
                }
            }
            _ = interval.tick() => {
                match poll_once(&mut client, &cli.chat, baseline.sent_message_id).await {
                    Ok(replies) if !replies.is_empty() => {
                        send_ack(&mut client, &cli.chat).await;
                        let reply_text = replies
                            .iter()
                            .filter_map(|r| r.text.as_deref())
                            .collect::<Vec<_>>()
                            .join("\n");
                        emit_tool_block(&questions.answer_from_reply(&reply_text));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        emit_status(&format!("tg-hook: poll failed (will retry): {e:#}"));
                    }
                }
            }
        }
    }
}
