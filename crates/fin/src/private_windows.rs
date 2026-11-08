//! Windows: whether a credential file is private to this user (sub-project 5A Task 9), the
//! counterpart of the Unix owner and mode check in [`crate::auth`].
//!
//! Read from the **opened handle** (`GetSecurityInfo`), so a file swapped in after the check
//! is not the one read. The file is private when its owner is the current user, `SYSTEM` or
//! the Administrators group (what an elevated process creates), its DACL is not NULL (FAT and
//! exFAT keep none), and every allow ACE with any access names the user, `SYSTEM`,
//! Administrators, `CREATOR OWNER` or `OWNER RIGHTS`: nothing for `Everyone`, `Users`,
//! `Authenticated Users` or another account. Deny ACEs are fine; an ACE type this does not
//! know is refused. These are the rules of `fin-registry`'s `private_windows.rs` (sub-project
//! 8A) and of `fin-node`'s `checkPrivateDir`; this small copy keeps `fin` free of either
//! crate. Problems name SIDs, never account names.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const ACCESS_DENIED_OBJECT_ACE_TYPE: u8 = 6;
const ACCESS_ALLOWED_CALLBACK_ACE_TYPE: u8 = 9;
const ACCESS_DENIED_CALLBACK_ACE_TYPE: u8 = 10;
const ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE: u8 = 12;
const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// `CREATOR OWNER` and `OWNER RIGHTS`: placeholders for the owner, who is checked.
const OWNER_ALIASES: [&str; 2] = ["S-1-3-0", "S-1-3-4"];

/// Frees a `LocalAlloc`ed pointer on drop.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a Win32 call that LocalAlloc'ed it.
            unsafe { LocalFree(self.0) };
        }
    }
}

fn sid_string(sid: PSID) -> Option<String> {
    let mut out: *mut u16 = null_mut();
    // SAFETY: `sid` is a valid SID owned by the caller; `out` receives a LocalAlloc'ed string.
    if unsafe { ConvertSidToStringSidW(sid, &mut out) } == 0 {
        return None;
    }
    let _free = Local(out.cast());
    let mut len = 0;
    // SAFETY: `out` is NUL-terminated.
    while unsafe { *out.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` u16s are readable at `out`.
    Some(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(out, len)
    }))
}

/// The current process token's user SID.
fn current_user() -> io::Result<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: plain FFI with valid out-pointers.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buf = vec![0u8; 256];
    let mut len = 0u32;
    // SAFETY: `buf` has `buf.len()` writable bytes; `token` is open.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &mut len,
        )
    };
    let err = io::Error::last_os_error();
    // SAFETY: `token` is open.
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err(err);
    }
    // SAFETY: GetTokenInformation(TokenUser) filled a TOKEN_USER at the start of `buf`.
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    sid_string(user.User.Sid).ok_or_else(io::Error::last_os_error)
}

/// Why the opened `file` is not private to this user (empty when it is): `"owner <sid>"`,
/// `"allows <sid>"`, `"null dacl"`, `"unsupported ace <type>"`, `"unreadable ace <i>"`.
pub(crate) fn problems(file: &File) -> io::Result<Vec<String>> {
    let user = current_user()?;
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the handle is open (with READ_CONTROL: std opens for reading with GENERIC_READ);
    // valid out-pointers; `sd` is LocalAlloc'ed.
    let rc = unsafe {
        GetSecurityInfo(
            file.as_raw_handle() as HANDLE,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    let _free = Local(sd);
    let trusted = |s: &str| s == user || s == SYSTEM || s == ADMINISTRATORS;
    let mut out = Vec::new();
    match sid_string(owner) {
        Some(o) if trusted(&o) => {}
        Some(o) => out.push(format!("owner {o}")),
        None => out.push("owner unknown".to_string()),
    }
    if dacl.is_null() {
        out.push("null dacl".to_string());
        return Ok(out);
    }
    // SAFETY: `dacl` points into `sd`, alive until `_free` drops.
    let count = unsafe { (*dacl).AceCount };
    for i in 0..u32::from(count) {
        let mut ace: *mut c_void = null_mut();
        // SAFETY: `i` < AceCount.
        if unsafe { GetAce(dacl, i, &mut ace) } == 0 {
            out.push(format!("unreadable ace {i}"));
            continue;
        }
        // SAFETY: every ACE starts with an ACE_HEADER.
        let header = unsafe { *(ace as *const ACE_HEADER) };
        match header.AceType {
            ACCESS_DENIED_ACE_TYPE
            | ACCESS_DENIED_OBJECT_ACE_TYPE
            | ACCESS_DENIED_CALLBACK_ACE_TYPE
            | ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE => {
                let allowed = ace as *const ACCESS_ALLOWED_ACE;
                // SAFETY: both types are laid out as ACCESS_ALLOWED_ACE up to `SidStart`.
                let (mask, sid) = unsafe {
                    (
                        (*allowed).Mask,
                        std::ptr::addr_of!((*allowed).SidStart) as PSID,
                    )
                };
                if mask == 0 {
                    continue;
                }
                match sid_string(sid) {
                    Some(s) if trusted(&s) || OWNER_ALIASES.contains(&s.as_str()) => {}
                    Some(s) => {
                        let p = format!("allows {s}");
                        if !out.contains(&p) {
                            out.push(p);
                        }
                    }
                    None => out.push(format!("unreadable ace {i}")),
                }
            }
            other => out.push(format!("unsupported ace {other}")),
        }
    }
    Ok(out)
}
