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
}

const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;

/// Walk the current process's ancestry, returning `[parent, grandparent, …]`
/// PIDs. Caps at 32 hops so a corrupt snapshot cannot produce an infinite
/// loop. Returns an empty `Vec` on any Win32 failure.
#[allow(
    clippy::cast_possible_truncation,
    reason = "dw_size is always small; isize cast is the canonical Win32 INVALID_HANDLE_VALUE check"
)]
#[allow(
    clippy::cast_possible_wrap,
    reason = "isize comparison with -1 is the canonical Win32 INVALID_HANDLE_VALUE check"
)]
pub fn pid_ancestry_chain() -> Vec<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot as isize == -1 {
        return Vec::new();
    }

    let mut entry: ProcessEntry32 = unsafe { std::mem::zeroed() };
    entry.dw_size = size_of::<ProcessEntry32>() as u32;
    let mut map: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut ok = unsafe { Process32FirstW(snapshot, &raw mut entry) };
    while ok != 0 {
        map.insert(entry.th32_process_id, entry.th32_parent_process_id);
        ok = unsafe { Process32NextW(snapshot, &raw mut entry) };
    }
    unsafe { CloseHandle(snapshot) };

    let mut chain = Vec::new();
    let mut cur = std::process::id();
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

/// Return the immediate parent PID of the current process, or `0` on failure.
pub fn parent_pid() -> u32 {
    pid_ancestry_chain().into_iter().next().unwrap_or(0)
}
