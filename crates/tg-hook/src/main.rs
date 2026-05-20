//! Claude Code Stop hook: bridges a Claude Code session to a Telegram
//! chat. See module docs in `lib.rs` for the high-level flow.

#![cfg(windows)]

use anyhow::{Context, Result, anyhow};
use std::time::Duration;
use tg_hook::cli::CliArgs;
use tg_hook::discovery::{load_all, pick_record, pid_chain};
use tg_hook::local_input::local_user_active;
use tg_hook::mcp_client::McpClient;
use tg_hook::output::{DEFAULT_RETRY_MESSAGE, emit_block, emit_status};
use tg_hook::poll::poll_once;
use tg_hook::stop_input::StopInput;
use tg_hook::wake::{send_ack, send_response_chunks, send_wakeup};

/// Poll interval for the local-input watcher — fast enough to release the
/// hook promptly once the user starts typing into the Claude Code window.
const LOCAL_INPUT_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() {
    // We deliberately swallow errors at the top level: any failure becomes
    // a non-blocking exit so Claude can stop normally. The user sees what
    // went wrong via stderr (Claude Code surfaces hook stderr).
    if let Err(e) = run().await {
        emit_status(&format!("tg-hook error: {e:#}"));
        std::process::exit(0);
    }
}

#[allow(
    clippy::cognitive_complexity,
    reason = "linear orchestration; splitting hurts readability"
)]
async fn run() -> Result<()> {
    let cli = CliArgs::parse_env()?;
    let stop_input = StopInput::from_stdin()?;

    // Process ancestry — used both for discovery-record matching and as
    // the allowlist for foreground-PID input detection.
    let chain = pid_chain();
    let local_threshold_ms: u32 =
        u32::try_from(cli.local_input_threshold_secs.saturating_mul(1000)).unwrap_or(u32::MAX);

    // Early exit if the user is already at the keyboard — skip the
    // Telegram wakeup and response mirror so we don't spam Telegram on
    // every turn when they're looking at Claude Code anyway.
    if cli.release_on_local_input && local_user_active(local_threshold_ms, &chain) {
        emit_status("tg-hook: local input detected at startup, skipping Telegram wakeup");
        return Ok(());
    }

    // 1. Locate the right TelegramMCP instance.
    let records = load_all().context("loading discovery records")?;
    if records.is_empty() {
        return Err(anyhow!(
            "no TelegramMCP discovery records found — is the MCP server running?"
        ));
    }
    let session_id_env = stop_input
        .session_id
        .clone()
        .or_else(|| std::env::var("CLAUDE_SESSION_ID").ok());
    let record = pick_record(&records, session_id_env.as_deref(), &chain)
        .ok_or_else(|| {
            anyhow!(
                "no TelegramMCP discovery record matched (session_id={session_id_env:?}, pid_chain={chain:?})"
            )
        })?
        .clone();

    // 2. Connect (verifying the pipe is served by the recorded PID).
    let mut client = McpClient::connect(&record.pipe, &record.token, record.pid)
        .await
        .context("connecting to TelegramMCP pipe")?;

    // 3. Send wakeup.
    let baseline = send_wakeup(&mut client, &cli.chat, &cli.message)
        .await
        .context("sending wakeup")?;

    // 4. Send the last assistant response as follow-up chunks.
    if let Some(ref text) = stop_input.last_assistant_message {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            send_response_chunks(&mut client, &cli.chat, trimmed).await;
        }
    }

    // 5. Announce intent on stderr so Claude Code shows it to the user.
    emit_status("Waiting on keyboard interrupt or external message");

    // 6. Race poll vs. timeout vs. ctrl-c vs. local input.
    let timeout = tokio::time::sleep(Duration::from_secs(cli.timeout_secs));
    tokio::pin!(timeout);
    let mut interval = tokio::time::interval(Duration::from_secs(cli.poll_secs));
    // `tokio::time::interval` fires immediately on its first tick; we want
    // a poll_secs delay before the first poll so the user has a chance to
    // see the message before we hammer SQLite.
    interval.tick().await;

    // Separate interval for the local-input watcher: poll faster so the
    // hook releases promptly when the user starts typing, but only when
    // the feature is opted in.
    let mut local_interval = tokio::time::interval(LOCAL_INPUT_POLL_INTERVAL);
    local_interval.tick().await;

    loop {
        tokio::select! {
            biased; // ctrl-c first — never let a poll mask a user-driven exit
            _ = tokio::signal::ctrl_c() => {
                emit_status("tg-hook: interrupted, releasing Claude");
                return Ok(()); // No `decision: block` -> Claude stops normally.
            }
            () = &mut timeout => {
                let msg = cli
                    .retry_message
                    .as_deref()
                    .unwrap_or(DEFAULT_RETRY_MESSAGE);
                emit_block(msg);
                return Ok(());
            }
            _ = local_interval.tick(), if cli.release_on_local_input => {
                if local_user_active(local_threshold_ms, &chain) {
                    emit_status("tg-hook: local input detected, releasing Claude");
                    return Ok(()); // Same exit as ctrl-c — let Claude stop.
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
                                    let t = r.text.as_deref()
                                        .unwrap_or("(media-only message)");
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
