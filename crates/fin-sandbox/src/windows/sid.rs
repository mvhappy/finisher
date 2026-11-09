//! An owned SID and the string conversions around it.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows_sys::Win32::Security::{GetLengthSid, IsValidSid, PSID};

/// A NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

/// The UTF-16 string at `p`, up to its NUL.
///
/// # Safety
/// `p` points to a NUL-terminated UTF-16 string.
pub unsafe fn from_wide(p: *const u16) -> String {
    let mut n = 0;
    // SAFETY: the caller's promise; stops at the NUL.
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `n` units were just read.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

/// A SID allocated by `LocalAlloc` (freed on drop).
#[derive(Debug)]
pub struct Sid(PSID);

// SAFETY: the SID is plain memory owned by this value alone.
unsafe impl Send for Sid {}
// SAFETY: as above; nothing mutates it after creation.
unsafe impl Sync for Sid {}

impl Sid {
    /// Parses `S-1-…`.
    pub fn parse(s: &str) -> io::Result<Sid> {
        let mut p: PSID = null_mut();
        // SAFETY: a NUL-terminated string and a valid out-pointer.
        if unsafe { ConvertStringSidToSidW(wide(s).as_ptr(), &mut p) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Sid(p))
    }

    /// A copy of the SID at `p` (which the caller keeps owning).
    ///
    /// # Safety
    /// `p` points to a valid SID.
    pub unsafe fn copy_of(p: PSID) -> io::Result<Sid> {
        // SAFETY: the caller's promise.
        Sid::parse(&unsafe { string_of(p) }?)
    }

    pub fn as_psid(&self) -> PSID {
        self.0
    }

    /// `S-1-…`.
    pub fn string(&self) -> String {
        // SAFETY: our own valid SID.
        unsafe { string_of(self.0) }.unwrap_or_else(|e| format!("<sid: {e}>"))
    }

    /// The SID's bytes.
    pub fn bytes(&self) -> Vec<u8> {
        // SAFETY: our own valid SID, `GetLengthSid` bytes long.
        unsafe {
            let n = GetLengthSid(self.0) as usize;
            std::slice::from_raw_parts(self.0.cast::<u8>(), n).to_vec()
        }
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: allocated by `ConvertStringSidToSidW` (LocalAlloc), freed once.
        unsafe { LocalFree(self.0 as HLOCAL) };
    }
}

/// The string form of the SID at `p`.
///
/// # Safety
/// `p` points to a SID.
pub unsafe fn string_of(p: PSID) -> io::Result<String> {
    // SAFETY: the caller's promise.
    if unsafe { IsValidSid(p) } == 0 {
        return Err(io::Error::other("not a valid SID"));
    }
    let mut s: *mut u16 = null_mut();
    // SAFETY: a valid SID and out-pointer.
    if unsafe { ConvertSidToStringSidW(p, &mut s) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a NUL-terminated string LocalAlloc'd by the call, freed once.
    let out = unsafe { from_wide(s) };
    // SAFETY: as above.
    unsafe { LocalFree(s as HLOCAL) };
    Ok(out)
}

/// The SID of the user this process runs as.
pub fn current_user() -> io::Result<Sid> {
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut tok = null_mut();
    // SAFETY: the pseudo-handle of this process and a valid out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let tok = super::owned(tok)?;
    let mut buf = vec![0u64; 64];
    let mut len = 0u32;
    // SAFETY: a token with TOKEN_QUERY and a buffer of the size given.
    let ok = unsafe {
        GetTokenInformation(
            super::raw(&tok),
            TokenUser,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call filled a TOKEN_USER whose SID lies in `buf`.
    unsafe { Sid::copy_of((*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid) }
}
