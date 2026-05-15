//! Claude Code Stop hook: bridges a Claude Code session to a Telegram
//! chat. See module docs in `lib.rs` for the high-level flow.

#![cfg(windows)]

use anyhow::{Context, Result, anyhow};
use std::time::Duration;
use tg_hook::cli::CliArgs;
use tg_hook::discovery::{load_all, pick_record, pid_chain};
use tg_hook::mcp_client::McpClient;
use tg_hook::output::{DEFAULT_RETRY_MESSAGE, emit_block, emit_status};
use tg_hook::poll::poll_once;
use tg_hook::stop_input::StopInput;
use tg_hook::wake::send_wakeup;

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
    let chain = pid_chain();
    let record = pick_record(&records, session_id_env.as_deref(), &chain)
        .ok_or_else(|| {
            anyhow!(
                "no TelegramMCP discovery record matched (session_id={session_id_env:?}, pid_chain={chain:?})"
            )
        })?
        .clone();

    // 2. Connect.
    let mut client = McpClient::connect(&record.pipe, &record.token)
        .await
        .context("connecting to TelegramMCP pipe")?;

    // 3. Send wakeup.
    let baseline = send_wakeup(&mut client, &cli.chat, &cli.message)
        .await
        .context("sending wakeup")?;

    // 4. Announce intent on stderr so Claude Code shows it to the user.
    emit_status("Waiting on keyboard interrupt or external message");

    // 5. Race poll vs. timeout vs. ctrl-c.
    let timeout = tokio::time::sleep(Duration::from_secs(cli.timeout_secs));
    tokio::pin!(timeout);
    let mut interval = tokio::time::interval(Duration::from_secs(cli.poll_secs));
    // `tokio::time::interval` fires immediately on its first tick; we want
    // a poll_secs delay before the first poll so the user has a chance to
    // see the message before we hammer SQLite.
    interval.tick().await;

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
            _ = interval.tick() => {
                match poll_once(&mut client, &cli.chat, baseline.sent_message_id).await {
                    Ok(Some(reply)) => {
                        let body = reply
                            .text
                            .unwrap_or_else(|| "(media-only message; no text)".into());
                        emit_block(&format!("Telegram reply: {body}"));
                        return Ok(());
                    }
                    Ok(None) => {}

                    Err(e) => {
                        // Transient errors shouldn't kill the wait — log and
                        // keep polling. A persistent failure surfaces via
                        // the timeout returning the retry-message.
                        emit_status(&format!("tg-hook: poll failed (will retry): {e:#}"));
                    }
                }
            }
        }
    }
}
