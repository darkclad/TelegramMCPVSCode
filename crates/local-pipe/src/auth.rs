//! Per-connection auth: read the first line, expect `AUTH <token>\n`.
//!
//! Implementation reads byte-by-byte from the raw pipe so no excess bytes
//! end up in a userland buffer — the post-auth stream is fully intact for
//! the caller (rmcp) to consume.

use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::net::windows::named_pipe::NamedPipeServer;

/// Errors surfaced by [`consume_auth_line`].
#[derive(Debug, Error)]
pub enum AuthError {
    /// I/O error reading the auth line.
    #[error("io error reading auth line: {0}")]
    Io(#[from] std::io::Error),
    /// Client closed the connection before completing the auth line.
    #[error("client closed before sending auth line")]
    Eof,
    /// Auth line did not start with the `AUTH ` prefix.
    #[error("malformed auth line; expected `AUTH <token>`")]
    Malformed,
    /// Token presented by the client did not match this server's token.
    #[error("token mismatch")]
    BadToken,
    /// Auth line exceeded the maximum allowed length.
    #[error("auth line exceeded {max} bytes")]
    TooLong {
        /// Maximum permitted length.
        max: usize,
    },
}

const MAX_AUTH_LINE_BYTES: usize = 1024;

/// Read `AUTH <token>\n` from the pipe and validate. On success returns the
/// pipe with no bytes pre-consumed past the newline — safe to hand to rmcp.
pub async fn consume_auth_line(
    mut pipe: NamedPipeServer,
    expected_token: &str,
) -> Result<NamedPipeServer, AuthError> {
    let mut line: Vec<u8> = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        let n = pipe.read(&mut byte).await?;
        if n == 0 {
            return Err(AuthError::Eof);
        }
        if byte[0] == b'\n' {
            break;
        }
        if line.len() >= MAX_AUTH_LINE_BYTES {
            return Err(AuthError::TooLong {
                max: MAX_AUTH_LINE_BYTES,
            });
        }
        line.push(byte[0]);
    }
    // Tolerate trailing CR (Windows line endings).
    if line.last() == Some(&b'\r') {
        line.pop();
    }

    let line_str = std::str::from_utf8(&line).map_err(|_| AuthError::Malformed)?;
    let Some(token) = line_str.strip_prefix("AUTH ") else {
        return Err(AuthError::Malformed);
    };

    if constant_time_eq(token.as_bytes(), expected_token.as_bytes()) {
        Ok(pipe)
    } else {
        Err(AuthError::BadToken)
    }
}

/// Constant-time byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn constant_time_eq_matches_equal_inputs() {
        assert!(constant_time_eq(b"deadbeef", b"deadbeef"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn constant_time_eq_rejects_differences() {
        assert!(!constant_time_eq(b"deadbeef", b"deadbee0")); // same length, last byte differs
        assert!(!constant_time_eq(b"deadbeef", b"deadbee")); // length mismatch
        assert!(!constant_time_eq(b"short", b"longer-token"));
        assert!(!constant_time_eq(b"abc", b""));
    }
}
