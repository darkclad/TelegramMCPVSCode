//! A restrictive Windows security descriptor for the named pipe.
//!
//! [`PipeSecurity::current_user_only`] builds a DACL that grants pipe access
//! only to the SID of the account running the MCP server. Other interactive
//! users on the machine are denied.
//!
//! This does **not** contain same-user code — a DACL cannot distinguish
//! processes of one user. It is defence-in-depth against the cross-user
//! case; the primary anti-tamper guarantees come from `first_pipe_instance`
//! (anti-squat) and the per-connection auth token.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;

/// Win32 `SECURITY_ATTRIBUTES`. Field order/types must match the C layout.
#[repr(C)]
struct SecurityAttributesRaw {
    n_length: u32,
    lp_security_descriptor: *mut c_void,
    b_inherit_handle: i32,
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn GetTokenInformation(
        token: *mut c_void,
        info_class: u32,
        info: *mut c_void,
        info_len: u32,
        return_len: *mut u32,
    ) -> i32;
    fn ConvertSidToStringSidW(sid: *mut c_void, string_sid: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        string_sd: *const u16,
        revision: u32,
        sd: *mut *mut c_void,
        sd_size: *mut u32,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

/// `TokenUser` value for `GetTokenInformation`'s information-class argument.
const TOKEN_USER_CLASS: u32 = 1;
/// `GetCurrentProcessToken()` pseudo-handle — a constant, no syscall needed.
const CURRENT_PROCESS_TOKEN: isize = -4;
/// `SDDL_REVISION_1` for the security-descriptor string parser.
const SDDL_REVISION_1: u32 = 1;

/// An owned Windows security descriptor restricting pipe access to one user.
///
/// Holds the `LocalAlloc`-backed descriptor returned by
/// `ConvertStringSecurityDescriptorToSecurityDescriptorW` and frees it on
/// drop.
pub struct PipeSecurity {
    descriptor: *mut c_void,
}

// SAFETY: `descriptor` is immutable after construction and only ever read by
// the kernel during pipe creation. Sharing it across threads (the accept
// loop runs as a spawned task) is therefore sound.
unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

impl PipeSecurity {
    /// Build a descriptor granting full access only to the SID of the user
    /// running this process.
    ///
    /// Returns `None` if any Win32 call fails — the caller then falls back to
    /// the default (process-token-derived) descriptor.
    #[must_use]
    pub fn current_user_only() -> Option<Self> {
        let sid_string = unsafe { current_user_sid_string()? };
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid_string})")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor: *mut c_void = ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 || descriptor.is_null() {
            return None;
        }
        Some(Self { descriptor })
    }

    /// A `SECURITY_ATTRIBUTES` value referencing this descriptor, ready to
    /// pass to `ServerOptions::create_with_security_attributes_raw`.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "struct size is a small compile-time constant"
    )]
    #[must_use]
    pub fn attributes(&self) -> SecurityAttributes {
        SecurityAttributes(SecurityAttributesRaw {
            n_length: size_of::<SecurityAttributesRaw>() as u32,
            lp_security_descriptor: self.descriptor,
            b_inherit_handle: 0,
        })
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe { LocalFree(self.descriptor) };
    }
}

/// A stack-resident `SECURITY_ATTRIBUTES`. Keep it alive across the pipe
/// `create` call and pass [`SecurityAttributes::as_mut_ptr`] into it.
pub struct SecurityAttributes(SecurityAttributesRaw);

impl SecurityAttributes {
    /// Raw pointer to the underlying `SECURITY_ATTRIBUTES` struct.
    pub fn as_mut_ptr(&mut self) -> *mut c_void {
        (&raw mut self.0).cast()
    }
}

/// Resolve the current process user's SID, formatted as an SDDL string
/// (e.g. `S-1-5-21-...`).
///
/// # Safety
///
/// Calls Win32 token APIs; all returned pointers are checked before use.
unsafe fn current_user_sid_string() -> Option<String> {
    let token = CURRENT_PROCESS_TOKEN as *mut c_void;

    // First call: probe the required buffer size.
    let mut needed: u32 = 0;
    unsafe {
        GetTokenInformation(token, TOKEN_USER_CLASS, ptr::null_mut(), 0, &raw mut needed);
    }
    if needed == 0 {
        return None;
    }

    // Over-allocate as u64 so the buffer is pointer-aligned for the
    // TOKEN_USER struct (whose first field is a PSID pointer).
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TOKEN_USER_CLASS,
            buf.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    };
    if ok == 0 {
        return None;
    }

    // TOKEN_USER begins with `SID_AND_ATTRIBUTES { PSID Sid; ... }` — the
    // first pointer-sized field is the SID pointer.
    let sid: *mut c_void = unsafe { buf.as_ptr().cast::<*mut c_void>().read() };
    if sid.is_null() {
        return None;
    }

    let mut sid_str: *mut u16 = ptr::null_mut();
    let ok = unsafe { ConvertSidToStringSidW(sid, &raw mut sid_str) };
    if ok == 0 || sid_str.is_null() {
        return None;
    }
    let result = unsafe { wide_to_string(sid_str) };
    unsafe { LocalFree(sid_str.cast()) };
    Some(result)
}

/// Read a NUL-terminated UTF-16 string into an owned [`String`].
///
/// # Safety
///
/// `ptr` must point to a NUL-terminated UTF-16 string.
unsafe fn wide_to_string(ptr: *const u16) -> String {
    let mut len = 0;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf16_lossy(slice)
}
