//! Win32 process-ancestry utilities shared by `local-pipe` and `tg-hook`.

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
    fn OpenProcess(desired_access: u32, inherit_handle: i32, pid: u32) -> *mut core::ffi::c_void;
    fn GetNamedPipeServerProcessId(pipe: *mut core::ffi::c_void, pid: *mut u32) -> i32;
}

/// Return the PID of the process serving the named pipe that `handle` is
/// connected to, or `None` on any Win32 failure.
///
/// Used by `tg-hook` to confirm a pipe is served by the process named in its
/// discovery record before speaking MCP over it.
#[must_use]
#[allow(
    clippy::not_unsafe_ptr_arg_deref,
    reason = "handle is an OS pipe handle passed by-value to a Win32 query; not dereferenced in Rust"
)]
pub fn named_pipe_server_pid(handle: std::os::windows::io::RawHandle) -> Option<u32> {
    let mut pid: u32 = 0;
    let ok = unsafe { GetNamedPipeServerProcessId(handle, &raw mut pid) };
    (ok != 0).then_some(pid)
}

/// Return `true` if a process with `pid` is currently alive.
pub fn process_alive(pid: u32) -> bool {
    // PROCESS_QUERY_LIMITED_INFORMATION — doesn't require admin rights.
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    unsafe { CloseHandle(handle) };
    true
}

const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;

/// Parent PID + lowercased executable name for one process in a snapshot.
#[derive(Debug, Clone)]
pub struct ProcInfo {
    /// Parent process id.
    pub parent: u32,
    /// Lowercased executable file name, e.g. `code.exe`.
    pub exe: String,
}

/// Decode a NUL-terminated UTF-16 `sz_exe_file` buffer into a lowercased name.
fn exe_name(buf: &[u16; 260]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len]).to_lowercase()
}

/// Snapshot every running process as `pid -> ProcInfo` (parent PID + exe
/// name). Returns an empty map on any Win32 failure.
#[allow(
    clippy::cast_possible_truncation,
    reason = "dw_size is always small; the u32 cast cannot lose data"
)]
#[allow(
    clippy::cast_possible_wrap,
    reason = "isize comparison with -1 is the canonical Win32 INVALID_HANDLE_VALUE check"
)]
pub fn process_snapshot() -> std::collections::HashMap<u32, ProcInfo> {
    let mut map = std::collections::HashMap::new();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot as isize == -1 {
        return map;
    }
    let mut entry: ProcessEntry32 = unsafe { std::mem::zeroed() };
    entry.dw_size = size_of::<ProcessEntry32>() as u32;
    let mut ok = unsafe { Process32FirstW(snapshot, &raw mut entry) };
    while ok != 0 {
        map.insert(
            entry.th32_process_id,
            ProcInfo {
                parent: entry.th32_parent_process_id,
                exe: exe_name(&entry.sz_exe_file),
            },
        );
        ok = unsafe { Process32NextW(snapshot, &raw mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    map
}

/// Walk `pid`'s ancestry within `snap`, returning `[pid, parent, grandparent,
/// …]` with the start `pid` included. Caps at 32 hops and stops on a cycle so
/// a corrupt snapshot cannot loop forever.
pub fn ancestry_in<S: std::hash::BuildHasher>(
    snap: &std::collections::HashMap<u32, ProcInfo, S>,
    pid: u32,
) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut cur = pid;
    for _ in 0..32 {
        let Some(info) = snap.get(&cur) else { break };
        let parent = info.parent;
        if parent == 0 || parent == cur || chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        cur = parent;
    }
    chain
}

/// Walk the current process's ancestry, returning `[parent, grandparent, …]`
/// PIDs (the current process itself is excluded). Empty on Win32 failure.
pub fn pid_ancestry_chain() -> Vec<u32> {
    let snap = process_snapshot();
    let mut chain = ancestry_in(&snap, std::process::id());
    if !chain.is_empty() {
        chain.remove(0);
    }
    chain
}

/// Return the immediate parent PID of the current process, or `0` on failure.
pub fn parent_pid() -> u32 {
    pid_ancestry_chain().into_iter().next().unwrap_or(0)
}
