//! Hand-rolled `--flag value` parser for tg-hook. Matches the style of
//! `mcp-server`'s `parse_cli`: rejects unknown flags so a settings.json
//! typo is loud rather than silently dropping behavior.

use anyhow::{Result, bail};

/// Parsed command-line arguments for tg-hook.
#[derive(Debug, Clone)]
pub struct CliArgs {
    /// Alias or numeric chat id to send the wakeup to.
    pub chat: String,
    /// Wakeup message for the `Stop` hook. Optional — defaults to a generic
    /// notice when omitted. The `AskUserQuestion` (`PreToolUse`) hook ignores
    /// this and builds its message from the question itself.
    pub message: Option<String>,
    /// Optional text returned to Claude when the `--timeout-secs` wait
    /// expires with no reply. Defaults to a generic retry notice when omitted.
    pub retry_message: Option<String>,
    /// How long to wait for a reply before returning the retry-message.
    /// Default 3600s (60 minutes), matching settings.json hook `timeout`.
    pub timeout_secs: u64,
    /// History-poll interval. Default 5s — small enough for snappy reply
    /// pickup, large enough not to hammer `SQLite`.
    pub poll_secs: u64,
    /// Release the hook when the local user is actively typing into the
    /// Claude Code host window. Off by default — opt in with
    /// `--release-on-local-input` in the hook command line.
    pub release_on_local_input: bool,
    /// How recent local input must be (seconds) to count as "user is at
    /// the keyboard". Default 2s. Only consulted when
    /// `release_on_local_input` is true.
    pub local_input_threshold_secs: u64,
}

impl CliArgs {
    /// Parse from an explicit argv vector (so tests can supply their own).
    ///
    /// # Errors
    ///
    /// Returns an error when required flags are absent, a flag is unknown,
    /// or a numeric flag fails to parse.
    pub fn parse_from(argv: Vec<String>) -> Result<Self> {
        let mut chat: Option<String> = None;
        let mut message: Option<String> = None;
        let mut retry_message: Option<String> = None;
        let mut timeout_secs: u64 = 3600;
        let mut poll_secs: u64 = 5;
        let mut release_on_local_input = false;
        let mut local_input_threshold_secs: u64 = 2;

        let mut it = argv.into_iter().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--chat" => {
                    chat = Some(
                        it.next()
                            .ok_or_else(|| anyhow::anyhow!("--chat needs a value"))?,
                    );
                }
                "--message" => {
                    message = Some(
                        it.next()
                            .ok_or_else(|| anyhow::anyhow!("--message needs a value"))?,
                    );
                }
                "--retry-message" => {
                    retry_message = Some(
                        it.next()
                            .ok_or_else(|| anyhow::anyhow!("--retry-message needs a value"))?,
                    );
                }
                "--timeout-secs" => {
                    let v = it
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--timeout-secs needs a value"))?;
                    timeout_secs = v
                        .parse()
                        .map_err(|_| anyhow::anyhow!("bad --timeout-secs"))?;
                }
                "--poll-secs" => {
                    let v = it
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--poll-secs needs a value"))?;
                    poll_secs = v.parse().map_err(|_| anyhow::anyhow!("bad --poll-secs"))?;
                }
                "--release-on-local-input" => {
                    release_on_local_input = true;
                }
                "--local-input-threshold-secs" => {
                    let v = it.next().ok_or_else(|| {
                        anyhow::anyhow!("--local-input-threshold-secs needs a value")
                    })?;
                    local_input_threshold_secs = v
                        .parse()
                        .map_err(|_| anyhow::anyhow!("bad --local-input-threshold-secs"))?;
                }
                "--help" | "-h" => {
                    eprintln!(
                        "tg-hook --chat <alias> [--message <text>] \
                         [--retry-message <text>] [--timeout-secs <int>] [--poll-secs <int>] \
                         [--release-on-local-input] [--local-input-threshold-secs <int>]"
                    );
                    std::process::exit(0);
                }
                other => bail!("unknown argument: {other}"),
            }
        }
        let chat = chat.ok_or_else(|| anyhow::anyhow!("--chat is required"))?;
        // Zero would make `tokio::time::interval` panic / the timeout fire
        // instantly — reject it loudly rather than crash the hook later.
        if poll_secs == 0 {
            bail!("--poll-secs must be at least 1");
        }
        if timeout_secs == 0 {
            bail!("--timeout-secs must be at least 1");
        }
        Ok(Self {
            chat,
            message,
            retry_message,
            timeout_secs,
            poll_secs,
            release_on_local_input,
            local_input_threshold_secs,
        })
    }

    /// Parse from `std::env::args`.
    ///
    /// # Errors
    ///
    /// See [`Self::parse_from`].
    pub fn parse_env() -> Result<Self> {
        Self::parse_from(std::env::args().collect())
    }
}
