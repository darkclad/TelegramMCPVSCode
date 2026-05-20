//! Pipe accept loop: per-connection auth then handoff to caller's serve fn.
//!
//! `run_pipe_server` owns the discovery-file lifecycle: write on entry,
//! remove on exit. It keeps accepting connections until the future is
//! dropped (e.g., main task exits).

use crate::auth::{AuthError, consume_auth_line};
use crate::discovery::{self, DiscoveryRecord};
use crate::security::PipeSecurity;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

/// Upper bound on the pipe create/connect retry backoff, in seconds.
const MAX_BACKOFF_SECS: u64 = 30;

/// Errors surfaced by [`run_pipe_server`].
#[derive(Debug, Error)]
pub enum PipeError {
    /// I/O error from creating the pipe or writing the discovery file.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Handler called for each authenticated connection. The pipe is positioned
/// immediately after the `AUTH <token>\n` line; everything from here on is
/// the caller's protocol (typically MCP JSON-RPC).
pub type ConnHandler =
    Arc<dyn Fn(NamedPipeServer) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Run the pipe server forever (or until the future is dropped).
///
/// Generates a per-instance pipe path + auth token, writes the discovery
/// file, and accepts connections. Each authenticated connection is handed
/// to `handler`.
///
/// The pipe is created with `first_pipe_instance` on the first instance (so
/// a squatter cannot pre-create the name), `reject_remote_clients` (so SMB
/// remote clients are refused), and — when it can be built — a DACL
/// restricting access to the current user.
///
/// The discovery file is removed when the returned future is dropped
/// (via the inner RAII guard).
///
/// # Errors
///
/// Returns [`PipeError::Io`] if the discovery file cannot be written, or if
/// the *first* pipe instance cannot be created — the latter means another
/// process already owns the pipe name (a squatter or a stale instance).
#[allow(
    clippy::similar_names,
    reason = "pid/ppid are standard OS terminology; renaming obscures intent"
)]
pub async fn run_pipe_server(handler: ConnHandler) -> Result<(), PipeError> {
    let pid = std::process::id();
    let ppid = crate::process::parent_pid();
    let pipe_path = format!(r"\\.\pipe\telegrammcp-{pid}");
    let token = uuid::Uuid::new_v4().simple().to_string();
    let session_id = std::env::var("CLAUDE_SESSION_ID").ok();

    let record = DiscoveryRecord {
        pid,
        ppid,
        pipe: pipe_path.clone(),
        token: token.clone(),
        session_id,
        started_at: now_iso8601(),
    };
    discovery::write(&record)?;
    let _guard = DiscoveryGuard(pid);

    // Restrict the pipe DACL to the current user. If the descriptor can't be
    // built we fall back to the process token's default DACL.
    let security = PipeSecurity::current_user_only();
    if security.is_none() {
        tracing::warn!("could not build restrictive pipe ACL; using default descriptor");
    }

    tracing::debug!(
        pipe = %pipe_path,
        ppid,
        secured = security.is_some(),
        "local-pipe server listening"
    );

    // Accept loop. Each connection: spawn a task that auth's + invokes handler.
    let token_for_loop = Arc::new(token);
    let mut first = true;
    let mut consecutive_failures = 0u32;
    loop {
        let server = match build_server(&pipe_path, first, security.as_ref()) {
            Ok(s) => {
                consecutive_failures = 0;
                s
            }
            Err(e) if first => {
                // The first instance must claim the pipe name exclusively.
                // A failure here means another process already owns it —
                // surface it instead of looping.
                tracing::error!(
                    error = %e,
                    pipe = %pipe_path,
                    "could not claim pipe name (squatted or stale instance?)"
                );
                return Err(PipeError::Io(e));
            }
            Err(e) => {
                tracing::warn!(error = %e, "pipe instance create failed");
                backoff_sleep(&mut consecutive_failures).await;
                continue;
            }
        };
        first = false;

        match server.connect().await {
            Ok(()) => {
                consecutive_failures = 0;
                let token = token_for_loop.clone();
                let handler = handler.clone();
                tokio::spawn(async move {
                    match consume_auth_line(server, &token).await {
                        Ok(pipe) => handler(pipe).await,
                        Err(AuthError::BadToken) => {
                            tracing::warn!("pipe client failed auth: bad token");
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "pipe client auth failure");
                        }
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "pipe connect() failed");
                backoff_sleep(&mut consecutive_failures).await;
            }
        }
    }
}

/// Create one pipe-server instance.
///
/// `first` claims the name exclusively via `first_pipe_instance`; it must be
/// set only on the very first instance. `security`, when present, restricts
/// the pipe DACL to the current user.
fn build_server(
    path: &str,
    first: bool,
    security: Option<&PipeSecurity>,
) -> std::io::Result<NamedPipeServer> {
    let mut opts = ServerOptions::new();
    opts.reject_remote_clients(true);
    if first {
        opts.first_pipe_instance(true);
    }
    match security {
        Some(sec) => {
            let mut attrs = sec.attributes();
            // SAFETY: `attrs` lives across the call, and the descriptor it
            // points at is owned by `sec` and valid for `sec`'s lifetime.
            unsafe { opts.create_with_security_attributes_raw(path, attrs.as_mut_ptr()) }
        }
        None => opts.create(path),
    }
}

/// Sleep with exponential-ish backoff (1s doubling, capped at
/// [`MAX_BACKOFF_SECS`]), bumping the consecutive-failure counter that drives
/// it. Keeps a misconfigured pipe from tight-looping and flooding the host.
async fn backoff_sleep(consecutive_failures: &mut u32) {
    *consecutive_failures = consecutive_failures.saturating_add(1);
    let secs = (1u64 << (*consecutive_failures).min(5)).min(MAX_BACKOFF_SECS);
    tracing::warn!(
        attempt = *consecutive_failures,
        backoff_secs = secs,
        "pipe server backing off"
    );
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

#[allow(
    clippy::cast_possible_wrap,
    reason = "unix timestamps fit in i64 for centuries"
)]
fn now_iso8601() -> String {
    // Format: YYYY-MM-DDTHH:MM:SSZ — UTC, no fractional seconds.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let secs = now as i64;
    let (year, month, day, hour, min, sec) = epoch_to_civil(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Howard Hinnant's date algorithm, simplified for UTC unix-epoch seconds.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "well-known date arithmetic; values stay in expected i64/u32 ranges"
)]
fn epoch_to_civil(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = (secs_of_day / 3600) as u32;
    let min = ((secs_of_day % 3600) / 60) as u32;
    let sec = (secs_of_day % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = (y + i64::from(m <= 2)) as i32;
    (y, m, d, hour, min, sec)
}

/// RAII guard that removes the discovery file when dropped.
struct DiscoveryGuard(u32);
impl Drop for DiscoveryGuard {
    fn drop(&mut self) {
        discovery::remove(self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::epoch_to_civil;

    #[test]
    fn epoch_to_civil_known_values() {
        assert_eq!(epoch_to_civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(epoch_to_civil(1_700_000_000), (2023, 11, 14, 22, 13, 20));
        // A leap day: 2020-02-29 00:00:00 UTC.
        assert_eq!(epoch_to_civil(1_582_934_400), (2020, 2, 29, 0, 0, 0));
        // End of day, last second.
        assert_eq!(epoch_to_civil(86_399), (1970, 1, 1, 23, 59, 59));
    }
}
