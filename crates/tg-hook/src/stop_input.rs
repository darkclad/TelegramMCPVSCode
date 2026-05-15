//! Stop-hook stdin payload sent by Claude Code.

use serde::Deserialize;

/// Stop-hook stdin payload. All fields optional so future Claude Code
/// versions can add fields without breaking the hook; unknown fields are
/// silently ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct StopInput {
    /// Unique id for the Claude Code session that fired this hook.
    /// Matched against `local-pipe`'s `DiscoveryRecord::session_id` first.
    pub session_id: Option<String>,
    /// Filesystem path of the Claude Code transcript. Unused today —
    /// kept to make future enrichment (e.g. quoting recent turns into the
    /// wakeup message) a one-line change rather than a re-parse.
    pub transcript_path: Option<String>,
    /// `true` when Claude has already been blocked by a Stop hook once and
    /// is firing the chain again. We deliberately ignore the value (we want
    /// the hook to keep blocking until reply or Ctrl+C), but accept the
    /// field so deserialization doesn't fail.
    pub stop_hook_active: Option<bool>,
    /// The last assistant message text from this turn. Sent to Telegram after
    /// the wakeup notification so the user can see what Claude produced.
    pub last_assistant_message: Option<String>,
}

impl StopInput {
    /// Read the JSON document from stdin. Returns `Ok(StopInput::default())`-
    /// equivalent (all-None) on empty stdin so the hook can also be smoke-
    /// tested by hand from a terminal.
    ///
    /// # Errors
    ///
    /// Returns an error if stdin contains non-empty but malformed JSON.
    pub fn from_stdin() -> Result<Self, std::io::Error> {
        use std::io::Read as _;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            return Ok(Self {
                session_id: None,
                transcript_path: None,
                stop_hook_active: None,
                last_assistant_message: None,
            });
        }
        serde_json::from_str(trimmed).map_err(std::io::Error::other)
    }
}
