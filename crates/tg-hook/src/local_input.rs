//! Detect when the local user is actively typing or moving the mouse with
//! the Claude Code host window focused. Used by `tg-hook` to release on
//! local activity in addition to a Telegram reply.
//!
//! Strategy: `GetLastInputInfo` reports system-wide input recency, which is
//! noisy on its own — moving the mouse over a browser shouldn't release the
//! hook. We pair it with `GetForegroundWindow` and require the focused
//! window to belong to the *same host application* as the hook.
//!
//! "Same application" is decided by process ancestry, not a single PID: the
//! hook and the window the user looks at are often different processes of
//! one app. A terminal Claude Code spawns the hook under the terminal
//! window's process. The VS Code extension is multi-process — the focused
//! editor window and the extension host that spawned the hook are sibling
//! `Code.exe` processes under a shared VS Code main process. So we find the
//! nearest common ancestor of the focused window and the hook, and treat the
//! user as present unless that ancestor is just the OS shell.

use local_pipe::{ProcInfo, ancestry_in, process_snapshot};
use std::collections::HashMap;
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

/// Watches for the local user becoming active at the Claude Code host
/// window.
///
/// Caches the process snapshot across calls: a live PID's ancestry never
/// changes, so a fresh snapshot is only needed when a window the cached
/// snapshot doesn't know gains focus. The hook polls this every ~500ms for
/// up to an hour, so reuse avoids thousands of full process-table walks.
pub struct LocalInputWatcher {
    /// The hook's ancestor PID chain — its "host application".
    host_pids: Vec<u32>,
    /// Input-recency threshold, in milliseconds.
    threshold_millis: u32,
    /// Cached process snapshot, reused while it still contains the focused
    /// window's PID.
    snapshot: HashMap<u32, ProcInfo>,
}

impl LocalInputWatcher {
    /// Create a watcher for a hook whose ancestor chain is `host_pids`,
    /// treating input within `threshold_millis` as "the user is present".
    pub fn new(host_pids: Vec<u32>, threshold_millis: u32) -> Self {
        Self {
            host_pids,
            threshold_millis,
            snapshot: HashMap::new(),
        }
    }

    /// Return `true` when system input has occurred within the threshold
    /// *and* the focused window belongs to the same host application as the
    /// hook (see [`focused_window_is_host`]).
    pub fn user_active(&mut self) -> bool {
        let Some(elapsed) = millis_since_input() else {
            return false;
        };
        if elapsed > self.threshold_millis {
            return false;
        }
        let fg = foreground_pid();
        if fg == 0 {
            return false;
        }
        // Re-snapshot only when the focused window is one the cached
        // snapshot doesn't know — a live PID's ancestry never changes.
        if !self.snapshot.contains_key(&fg) {
            self.snapshot = process_snapshot();
        }
        focused_window_is_host(fg, &self.host_pids, &self.snapshot)
    }
}

/// Decide whether the window owned by `fg_pid` belongs to the same host
/// application as the hook, whose ancestor PIDs are `host_pids`.
///
/// Walks the focused window's ancestry outward and takes the first process
/// that is also a hook ancestor — their nearest common ancestor. The window
/// counts as "the host" unless that ancestor is a generic OS process (the
/// shell or a service host), which is all an *unrelated* app would share
/// with the hook.
fn focused_window_is_host(fg_pid: u32, host_pids: &[u32], snap: &HashMap<u32, ProcInfo>) -> bool {
    for pid in ancestry_in(snap, fg_pid) {
        if host_pids.contains(&pid) {
            return !is_generic_ancestor(snap, pid);
        }
    }
    false
}

/// `true` for processes that unrelated applications share as a common
/// ancestor — the OS shell, service hosts, the system root. A nearest common
/// ancestor landing here means the focused window is a *different*
/// application, not the Claude Code host.
fn is_generic_ancestor(snap: &HashMap<u32, ProcInfo>, pid: u32) -> bool {
    if pid <= 4 {
        return true; // System Idle Process / System
    }
    match snap.get(&pid) {
        None => true,
        Some(info) => matches!(
            info.exe.as_str(),
            "explorer.exe"
                | "svchost.exe"
                | "services.exe"
                | "wininit.exe"
                | "winlogon.exe"
                | "userinit.exe"
                | "runtimebroker.exe"
                | ""
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(parent: u32, exe: &str) -> ProcInfo {
        ProcInfo {
            parent,
            exe: exe.to_string(),
        }
    }

    /// A VS Code-extension process tree:
    /// ```text
    ///   4   System
    ///   100 explorer.exe        (parent 4)
    ///   200 code.exe            (VS Code main,        parent 100)
    ///   210 code.exe            (focused renderer,   parent 200)
    ///   220 code.exe            (extension host,     parent 200)
    ///   230 cmd.exe             (the .bat wrapper,   parent 220)
    ///   240 tg-hook.exe         (the hook,           parent 230)
    ///   300 firefox.exe         (unrelated app,      parent 100)
    /// ```
    fn vscode_snapshot() -> HashMap<u32, ProcInfo> {
        HashMap::from([
            (4, proc(0, "system")),
            (100, proc(4, "explorer.exe")),
            (200, proc(100, "code.exe")),
            (210, proc(200, "code.exe")),
            (220, proc(200, "code.exe")),
            (230, proc(220, "cmd.exe")),
            (240, proc(230, "tg-hook.exe")),
            (300, proc(100, "firefox.exe")),
        ])
    }

    /// The hook's ancestor chain (excludes the hook itself, pid 240).
    fn hook_chain() -> Vec<u32> {
        vec![230, 220, 200, 100, 4]
    }

    #[test]
    fn focused_vscode_window_counts_as_host() {
        // Renderer (210) and the hook share VS Code main (200) — not generic.
        assert!(focused_window_is_host(
            210,
            &hook_chain(),
            &vscode_snapshot()
        ));
    }

    #[test]
    fn focused_unrelated_app_is_not_host() {
        // Firefox (300) shares only explorer.exe with the hook.
        assert!(!focused_window_is_host(
            300,
            &hook_chain(),
            &vscode_snapshot()
        ));
    }

    #[test]
    fn focused_explorer_window_is_not_host() {
        assert!(!focused_window_is_host(
            100,
            &hook_chain(),
            &vscode_snapshot()
        ));
    }

    #[test]
    fn terminal_window_in_hook_chain_counts_as_host() {
        // Terminal Claude Code: the focused terminal window *is* an ancestor
        // of the hook.
        let snap = HashMap::from([
            (4, proc(0, "system")),
            (100, proc(4, "explorer.exe")),
            (500, proc(100, "windowsterminal.exe")),
            (510, proc(500, "powershell.exe")),
            (520, proc(510, "tg-hook.exe")),
        ]);
        let chain = vec![510, 500, 100, 4];
        assert!(focused_window_is_host(500, &chain, &snap));
    }

    #[test]
    fn unknown_foreground_pid_is_not_host() {
        assert!(!focused_window_is_host(
            9999,
            &hook_chain(),
            &vscode_snapshot()
        ));
    }
}
