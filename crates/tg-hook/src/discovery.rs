//! Find the `TelegramMCP` `DiscoveryRecord` belonging to *this* Claude Code
//! session.

use local_pipe::{DiscoveryRecord, discovery::discovery_dir};

/// Scan the discovery directory and load every parseable record.
///
/// Files that fail to parse are skipped silently — a half-written record
/// (e.g. an older server format) should not crash the hook.
///
/// # Errors
///
/// Returns an error only when the discovery directory itself cannot be
/// read; per-file IO errors are tolerated.
pub fn load_all() -> std::io::Result<Vec<DiscoveryRecord>> {
    let dir = discovery_dir()?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        if let Ok(rec) = serde_json::from_slice::<DiscoveryRecord>(&bytes) {
            out.push(rec);
        }
    }
    Ok(out)
}

/// Pick the right record for this hook process.
///
/// Match priority:
/// 1. `session_id_env == record.session_id` (when both present).
/// 2. The first record whose `ppid` appears anywhere in `pid_chain`,
///    walking from the hook's immediate parent upward. The "first match
///    walking up" rule keeps things deterministic when the hook runs
///    under a wrapper (the wrapper's pid will hit before Claude's).
pub fn pick_record<'a>(
    records: &'a [DiscoveryRecord],
    session_id_env: Option<&str>,
    pid_chain: &[u32],
) -> Option<&'a DiscoveryRecord> {
    if let Some(sid) = session_id_env {
        if let Some(r) = records
            .iter()
            .find(|r| r.session_id.as_deref() == Some(sid))
        {
            return Some(r);
        }
    }
    for pid in pid_chain {
        if let Some(r) = records.iter().find(|r| r.ppid == *pid) {
            return Some(r);
        }
    }
    None
}

/// Walk the current process's ancestry, returning `[parent, grandparent, …]`
/// PIDs. Delegates to the shared Win32 implementation in `local-pipe`.
pub fn pid_chain() -> Vec<u32> {
    local_pipe::pid_ancestry_chain()
}
