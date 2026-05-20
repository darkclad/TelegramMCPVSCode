//! Find the `TelegramMCP` `DiscoveryRecord` belonging to *this* Claude Code
//! session.

use local_pipe::{DiscoveryRecord, discovery::discovery_dir, process_alive};

/// Scan the discovery directory and return only records whose server process
/// is still alive. Stale files (dead processes) are removed as a side effect.
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
        let path = entry.path();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(rec) = serde_json::from_slice::<DiscoveryRecord>(&bytes) else {
            // Unparseable: either half-written or an incompatible older
            // format. Remove it so a permanently-corrupt file doesn't linger.
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if process_alive(rec.pid) {
            out.push(rec);
        } else {
            // Remove the stale file so it doesn't accumulate across restarts.
            let _ = std::fs::remove_file(&path);
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
    // Last resort: if exactly one live server exists, it must be ours.
    if records.len() == 1 {
        return Some(&records[0]);
    }
    None
}

/// Walk the current process's ancestry, returning `[parent, grandparent, …]`
/// PIDs. Delegates to the shared Win32 implementation in `local-pipe`.
pub fn pid_chain() -> Vec<u32> {
    local_pipe::pid_ancestry_chain()
}
