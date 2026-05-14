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
/// the reply the next turn.
pub fn emit_block(reason: &str) {
    let payload = json!({ "decision": "block", "reason": reason });
    println!("{payload}");
}

/// Default retry-message used when `--retry-message` is omitted.
pub const DEFAULT_RETRY_MESSAGE: &str =
    "No Telegram reply within the wait window. Decide whether to wait again or wrap up.";
