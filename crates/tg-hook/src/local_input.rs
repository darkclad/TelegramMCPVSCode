//! Detect when the local user is actively typing or moving the mouse with
//! the Claude Code host window focused. Used by `tg-hook` to release on
//! local activity in addition to a Telegram reply.
//!
//! Strategy: `GetLastInputInfo` reports system-wide input recency, which is
//! noisy on its own — moving the mouse over a browser shouldn't release
//! the hook. We pair it with `GetForegroundWindow` and require the
//! foreground PID to belong to the hook's ancestor chain (terminal,
//! Claude Code, etc.). Together that means "input happened *and* the user
//! is looking at the window we care about".

use std::ffi::c_void;

#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, lpdw_process_id: *mut u32) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
}

/// Milliseconds since the last system-wide input event. `None` on Win32
/// failure. Uses wrapping subtraction so it stays correct across the
/// 49.7-day `GetTickCount` rollover.
fn millis_since_input() -> Option<u32> {
    let cb_size = u32::try_from(std::mem::size_of::<LastInputInfo>()).ok()?;
    let mut info = LastInputInfo {
        cb_size,
        dw_time: 0,
    };
    let ok = unsafe { GetLastInputInfo(&raw mut info) };
    if ok == 0 {
        return None;
    }
    let now = unsafe { GetTickCount() };
    Some(now.wrapping_sub(info.dw_time))
}

/// PID of the process owning the currently-focused window. `0` on failure.
fn foreground_pid() -> u32 {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return 0;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &raw mut pid) };
    pid
}

/// Return `true` when system input has occurred within `threshold_millis`
/// *and* the foreground window belongs to a process in `allowed_pids`
/// (typically the hook's ancestor chain). The PID gate is what keeps a
/// stray mouse jiggle on an unrelated window from releasing the hook.
pub fn local_user_active(threshold_millis: u32, allowed_pids: &[u32]) -> bool {
    let Some(elapsed) = millis_since_input() else {
        return false;
    };
    if elapsed > threshold_millis {
        return false;
    }
    let fg = foreground_pid();
    fg != 0 && allowed_pids.contains(&fg)
}
