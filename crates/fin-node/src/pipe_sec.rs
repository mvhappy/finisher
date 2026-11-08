//! The security pieces of the Windows pipe listener ([`crate::pipe_win`], sub-project 5A
//! Task 11, Decision 38): the pipe's security descriptor, SIDs as strings, and who a client is.
//!
//! - **The descriptor** is `D:P(A;;GA;;;<user SID>)(A;;GA;;;SY)` plus one
//!   `(A;;0x12008b;;;<sid>)` per allowed SID (sub-project 7: AppContainer SIDs). The allowed
//!   SIDs get `FILE_GENERIC_READ | FILE_WRITE_DATA`, not the plan's `GRGW`: on a pipe
//!   `FILE_APPEND_DATA` (part of `GENERIC_WRITE`) is `FILE_CREATE_PIPE_INSTANCE`, which would
//!   let a sandboxed process create an instance of the server's own pipe and receive the next
//!   client. Clients therefore open the pipe with `GENERIC_READ | FILE_WRITE_DATA` (`fin` does).
//!   An AppContainer runs at low integrity, and a pipe without a label counts as medium with
//!   no write-up, so once any SID is allowed the descriptor also carries the low label
//!   `S:(ML;;NW;;;LW)`; the identity check then refuses every low-integrity client that is not
//!   an allowed container.
//! - **A client** is identified by its process (`GetNamedPipeClientProcessId`) and that
//!   process's token: the user SID, its integrity level and, for a sandboxed process, its
//!   AppContainer SID. A token that cannot be read is a refusal (fail closed).

use std::ffi::c_void;
use std::io;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    ConvertStringSidToSidW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetSecurityDescriptorDacl, GetSecurityDescriptorSacl, GetTokenInformation,
    PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TokenAppContainerSid, TokenIntegrityLevel, TokenUser,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// What an allowed SID may do on the pipe: `FILE_GENERIC_READ | FILE_WRITE_DATA` (module docs).
pub(crate) const CLIENT_RIGHTS: u32 = 0x0012_008B;
/// `SECURITY_MANDATORY_MEDIUM_RID`: the least a client that is not an allowed container needs.
pub(crate) const MEDIUM_INTEGRITY: u32 = 0x2000;

/// `s` as a NUL-terminated UTF-16 string.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Frees a `LocalAlloc`ed pointer on drop.
pub(crate) struct Local(pub *mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from an API documented to `LocalAlloc` it.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// Closes a handle on drop.
pub(crate) struct Owned(pub HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle is owned by this value and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// A SID as `S-1-…`.
pub(crate) fn sid_string(sid: PSID) -> io::Result<String> {
    let mut out: *mut u16 = null_mut();
    // SAFETY: `sid` points at a valid SID; `out` receives a LocalAlloc'ed string.
    if unsafe { ConvertSidToStringSidW(sid, &mut out) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _free = Local(out.cast());
    let mut len = 0;
    // SAFETY: `out` is NUL-terminated.
    while unsafe { *out.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` u16s were just read.
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(out, len)
    }))
}

/// `sid` in canonical form (`S-1-…`, upper case), or `InvalidInput` when it is not a SID.
pub(crate) fn canonical_sid(sid: &str) -> io::Result<String> {
    let w = wide(sid);
    let mut bin: PSID = null_mut();
    // SAFETY: `w` is NUL-terminated; `bin` receives a LocalAlloc'ed SID.
    if unsafe { ConvertStringSidToSidW(w.as_ptr(), &mut bin) } == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a SID: {sid}"),
        ));
    }
    let _free = Local(bin);
    sid_string(bin)
}

/// One token information class, in a buffer aligned for the structures it holds.
fn token_info(token: HANDLE, class: i32) -> io::Result<Vec<u64>> {
    let mut len = 0u32;
    // SAFETY: a size query (null buffer, zero length).
    unsafe { GetTokenInformation(token, class, null_mut(), 0, &mut len) };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes.
    if unsafe { GetTokenInformation(token, class, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf)
}

/// Who a token belongs to: its user SID and, for an AppContainer token, the container's SID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub user: String,
    pub app_container: Option<String>,
    /// The mandatory integrity level's RID (`0x2000` medium, `0x3000` high; 0 when unknown).
    pub integrity: u32,
}

/// The identity of `token` (opened with `TOKEN_QUERY`).
pub(crate) fn token_identity(token: HANDLE) -> io::Result<Identity> {
    let user = token_info(token, TokenUser)?;
    // SAFETY: `TokenUser` fills a `TOKEN_USER` (a `SID_AND_ATTRIBUTES`: the SID pointer first).
    let user_sid = unsafe { *user.as_ptr().cast::<PSID>() };
    let user = sid_string(user_sid)?;
    let ac = token_info(token, TokenAppContainerSid)?;
    // SAFETY: `TokenAppContainerSid` fills a `TOKEN_APPCONTAINER_INFORMATION` (one SID
    // pointer, null for a token that is not an AppContainer's).
    let ac_sid = unsafe { *ac.as_ptr().cast::<PSID>() };
    let app_container = if ac_sid.is_null() {
        None
    } else {
        Some(sid_string(ac_sid)?)
    };
    let label = token_info(token, TokenIntegrityLevel)?;
    // SAFETY: `TokenIntegrityLevel` fills a `TOKEN_MANDATORY_LABEL` (a `SID_AND_ATTRIBUTES`).
    let label = sid_string(unsafe { *label.as_ptr().cast::<PSID>() })?;
    let integrity = label
        .strip_prefix("S-1-16-")
        .and_then(|rid| rid.parse().ok())
        .unwrap_or(0);
    Ok(Identity {
        user,
        app_container,
        integrity,
    })
}

/// The identity of this process.
pub(crate) fn current_identity() -> io::Result<Identity> {
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo handle of this process; `token` receives a handle we close.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Owned(token);
    token_identity(token.0)
}

/// The identity of process `pid` (a pipe client). Fails for a process this user cannot query.
pub(crate) fn process_identity(pid: u32) -> io::Result<Identity> {
    // SAFETY: plain calls; the handles are closed by `Owned`.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Owned(process);
    let mut token: HANDLE = null_mut();
    // SAFETY: as above.
    if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Owned(token);
    token_identity(token.0)
}

/// Whether a client with identity `peer` may stay connected: its user is ours (or an allowed
/// SID), and it is an allowed AppContainer or runs at medium integrity or above. The DACL
/// refuses the rest first; this is the second check of the spec's two layers.
pub(crate) fn peer_allowed(ours: &str, allow: &[String], peer: &Identity) -> bool {
    let user_ok = peer.user == ours || allow.contains(&peer.user);
    let sandbox_ok = match &peer.app_container {
        Some(ac) => allow.contains(ac),
        None => peer.integrity >= MEDIUM_INTEGRITY,
    };
    user_ok && sandbox_ok
}

/// The pipe's SDDL (module docs).
pub(crate) fn sddl(user: &str, allow: &[String]) -> String {
    let mut s = format!("D:P(A;;GA;;;{user})(A;;GA;;;SY)");
    for sid in allow {
        s.push_str(&format!("(A;;{CLIENT_RIGHTS:#x};;;{sid})"));
    }
    if !allow.is_empty() {
        s.push_str("S:(ML;;NW;;;LW)");
    }
    s
}

/// A self-relative security descriptor made from SDDL.
pub(crate) struct Descriptor(Local);

// SAFETY: the descriptor is plain memory owned by this value; it is only read after creation.
unsafe impl Send for Descriptor {}

impl Descriptor {
    pub(crate) fn from_sddl(sddl: &str) -> io::Result<Descriptor> {
        let w = wide(sddl);
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `w` is NUL-terminated; `sd` receives a LocalAlloc'ed descriptor.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Descriptor(Local(sd)))
    }

    pub(crate) fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0.0
    }

    /// The descriptor's SACL (its mandatory label), if it has one.
    pub(crate) fn sacl(&self) -> *mut windows_sys::Win32::Security::ACL {
        let (mut present, mut defaulted) = (0, 0);
        let mut acl = null_mut();
        // SAFETY: as in `dacl`.
        let ok = unsafe {
            GetSecurityDescriptorSacl(self.as_ptr(), &mut present, &mut acl, &mut defaulted)
        };
        if ok == 0 || present == 0 {
            null_mut()
        } else {
            acl
        }
    }

    /// The descriptor's DACL (owned by the descriptor).
    pub(crate) fn dacl(&self) -> io::Result<*mut windows_sys::Win32::Security::ACL> {
        let (mut present, mut defaulted) = (0, 0);
        let mut acl = null_mut();
        // SAFETY: a valid descriptor; the out-pointers are locals.
        let ok = unsafe {
            GetSecurityDescriptorDacl(self.as_ptr(), &mut present, &mut acl, &mut defaulted)
        };
        if ok == 0 || present == 0 || acl.is_null() {
            return Err(io::Error::other("the pipe descriptor has no DACL"));
        }
        Ok(acl)
    }
}
