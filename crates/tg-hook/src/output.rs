//! Stop-hook output helpers: the final stdout JSON (consumed by Claude
//! Code), and the in-flight stderr status line shown to the user.

use serde_json::json;
use std::io::Write as _;

/// Print the in-progress status to stderr so Claude Code surfaces it to
/// the user while the hook is blocked.
pub fn emit_status(message: &str) {
    // Tolerate a closed stderr (rare; some CI hosts redirect to null).
    let _ = writeln!(std::io::stderr(), "{message}");
    let _ = std::io::stderr().flush();
}

/// Print `{"decision":"block","reason":"..."}` to stdout. Claude Code re-
/// invokes the model with `reason` as a system note, effectively making
/// the reply the next turn. Used by the `Stop` hook.
pub fn emit_block(reason: &str) {
    let payload = json!({ "decision": "block", "reason": reason });
    println!("{payload}");
}

/// Block an `AskUserQuestion` tool call, handing `reason` to Claude.
///
/// A `PreToolUse` hook that exits with status 2 blocks the tool, and Claude
/// Code feeds the hook's stderr to the model as the blocking reason — that
/// is how a Telegram-relayed answer reaches Claude. Does not return.
pub fn emit_tool_block(reason: &str) -> ! {
    let _ = writeln!(std::io::stderr(), "{reason}");
    let _ = std::io::stderr().flush();
    std::process::exit(2);
}

/// Default retry-message used when `--retry-message` is omitted.
pub const DEFAULT_RETRY_MESSAGE: &str =
    "No Telegram reply within the wait window. Decide whether to wait again or wrap up.";

/// Default wakeup message used by the `Stop` hook when `--message` is omitted.
pub const DEFAULT_WAKEUP_MESSAGE: &str =
    "Claude finished its turn — reply here to continue, or press Ctrl+C in Claude Code to release.";

/// Acknowledgement sent to Telegram the moment an inbound reply is detected,
/// so the user sees that Claude received their message.
pub const DEFAULT_ACK_MESSAGE: &str = "Got it, working on it...";
