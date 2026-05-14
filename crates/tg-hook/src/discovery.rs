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

/// Walk the current process's ancestry on Windows, returning [parent,
/// grandparent, ...] PIDs. Mirrors the `parent_pid` walk in `local-pipe`
/// but iterates the snapshot multiple times to build a chain.
///
/// Returns an empty vector on any Win32 failure — callers fall back to
/// `session_id` matching, which is the more robust signal anyway.
#[cfg(windows)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "dw_size is always small; isize-to-pointer cast is the canonical Win32 INVALID_HANDLE_VALUE check"
)]
#[allow(
    clippy::cast_possible_wrap,
    reason = "isize comparison with -1 is the canonical Win32 INVALID_HANDLE_VALUE check"
)]
pub fn pid_chain() -> Vec<u32> {
    use std::mem::size_of;

    #[repr(C)]
    struct ProcessEntry32 {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut core::ffi::c_void;
        fn Process32FirstW(snapshot: *mut core::ffi::c_void, entry: *mut ProcessEntry32) -> i32;
        fn Process32NextW(snapshot: *mut core::ffi::c_void, entry: *mut ProcessEntry32) -> i32;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;

    let mut chain = Vec::new();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot as isize == -1 {
        return chain;
    }

    // Build pid -> ppid map once so we don't re-snapshot per hop.
    let mut entry: ProcessEntry32 = unsafe { std::mem::zeroed() };
    entry.dw_size = size_of::<ProcessEntry32>() as u32;
    let mut map: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut ok = unsafe { Process32FirstW(snapshot, &raw mut entry) };
    while ok != 0 {
        map.insert(entry.th32_process_id, entry.th32_parent_process_id);
        ok = unsafe { Process32NextW(snapshot, &raw mut entry) };
    }
    unsafe { CloseHandle(snapshot) };

    let mut cur = std::process::id();
    // Cap depth so a corrupt snapshot can't loop us.
    for _ in 0..32 {
        let Some(&ppid) = map.get(&cur) else { break };
        if ppid == 0 || ppid == cur {
            break;
        }
        chain.push(ppid);
        cur = ppid;
    }
    chain
}

#[cfg(not(windows))]
pub fn pid_chain() -> Vec<u32> {
    Vec::new()
}
