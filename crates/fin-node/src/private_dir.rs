//! Private directories for the server's data (sub-project 5A, spec §4.11): `checkPrivateDir`
//! and `makePrivateDir`.
//!
//! - **Windows:** a directory is private when its owner is the current user (or the
//!   Administrators group, which owns what an elevated process creates) and its DACL allows
//!   nobody but that user, `SYSTEM`, Administrators and `CREATOR OWNER`: no allow ACE for
//!   `Everyone` (S-1-1-0), `Users` (S-1-5-32-545), `Authenticated Users` (S-1-5-11) or any
//!   other account, and no NULL DACL. `makePrivateDir` creates the directory and sets a
//!   protected DACL `D:P(A;OICI;FA;;;<user SID>)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)` (inherited
//!   ACEs dropped) with the user as owner, so files created inside inherit it.
//! - **Elsewhere:** POSIX modes do the job (`finisherd` checks the owner uid and strips
//!   group/other bits itself): `checkPrivateDir` reports ok and `makePrivateDir` is
//!   `mkdir -p` plus `chmod 0700`.
//!
//! Reports name SIDs, never account names.

use fin_core::CoreError;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::json;

use crate::{guarded, reject};

/// What `checkPrivateDir` found.
#[napi(object)]
pub struct PrivateDirReport {
    pub ok: bool,
    /// The owner's SID (Windows only).
    pub owner: Option<String>,
    /// Why it is not private: `"owner <sid>"`, `"allows <sid>"`, `"null dacl"`,
    /// `"unsupported ace <type>"`.
    pub problems: Vec<String>,
}

fn io_error(path: &str, what: &str, e: &std::io::Error) -> Error {
    reject(
        CoreError::new("SERVER_DATA_UNSAFE", format!("cannot {what} {path}: {e}"))
            .with_details(json!({ "path": path, "cause": e.kind().to_string() })),
    )
}

/// Checks that `path` is private to this user (module docs). A missing path is an error.
#[napi(catch_unwind)]
pub fn check_private_dir(path: String) -> Result<PrivateDirReport> {
    guarded(|| imp::check(&path), || {})
}

/// Creates `path` (and parents) and makes it private to this user (module docs).
#[napi(catch_unwind)]
pub fn make_private_dir(path: String) -> Result<()> {
    guarded(
        || {
            std::fs::create_dir_all(&path).map_err(|e| io_error(&path, "create", &e))?;
            imp::make(&path)
        },
        || {},
    )
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    #[allow(clippy::disallowed_methods)] // the server's private directories
    pub fn check(path: &str) -> Result<PrivateDirReport> {
        std::fs::metadata(path).map_err(|e| io_error(path, "inspect", &e))?;
        Ok(PrivateDirReport {
            ok: true,
            owner: None,
            problems: Vec::new(),
        })
    }

    pub fn make(path: &str) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| io_error(path, "chmod", &e))
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
        GetSecurityDescriptorDacl, GetTokenInformation, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::*;

    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const ACCESS_DENIED_ACE_TYPE: u8 = 1;
    const ACCESS_ALLOWED_CALLBACK_ACE_TYPE: u8 = 9;
    const ACCESS_DENIED_CALLBACK_ACE_TYPE: u8 = 10;
    const ACCESS_DENIED_OBJECT_ACE_TYPE: u8 = 6;
    const ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE: u8 = 12;
    /// `SYSTEM`, Administrators, `CREATOR OWNER` (inherit-only placeholders for the owner).
    const ALLOWED: [&str; 3] = ["S-1-5-18", "S-1-5-32-544", "S-1-3-0"];
    const ADMINISTRATORS: &str = "S-1-5-32-544";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn win_error(path: &str, what: &str, code: u32) -> Error {
        io_error(
            path,
            what,
            &std::io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX)),
        )
    }

    /// Frees a `LocalAlloc`ed pointer on drop.
    struct Local(*mut c_void);
    impl Drop for Local {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from an API documented to `LocalAlloc` it.
                unsafe { LocalFree(self.0) };
            }
        }
    }

    /// A SID as `S-1-…`.
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
    fn current_user(path: &str) -> Result<String> {
        let mut token: HANDLE = null_mut();
        // SAFETY: plain FFI with valid out-pointers.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io_error(
                path,
                "read the process token for",
                &std::io::Error::last_os_error(),
            ));
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
        let err = std::io::Error::last_os_error();
        // SAFETY: `token` is open.
        unsafe { CloseHandle(token) };
        if ok == 0 {
            return Err(io_error(path, "read the user of", &err));
        }
        // SAFETY: GetTokenInformation(TokenUser) filled a TOKEN_USER at the start of `buf`.
        let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
        sid_string(user.User.Sid).ok_or_else(|| {
            io_error(
                path,
                "format the user SID for",
                &std::io::Error::last_os_error(),
            )
        })
    }

    pub fn check(path: &str) -> Result<PrivateDirReport> {
        let user = current_user(path)?;
        let name = wide(path);
        let mut owner: PSID = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: valid NUL-terminated name and out-pointers; `sd` is LocalAlloc'ed.
        let rc = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
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
            return Err(win_error(path, "read the security of", rc));
        }
        let _free = Local(sd);
        let mut problems = Vec::new();
        let owner_sid = sid_string(owner);
        match owner_sid.as_deref() {
            Some(o) if o == user || o == ADMINISTRATORS => {}
            Some(o) => problems.push(format!("owner {o}")),
            None => problems.push("owner unknown".to_string()),
        }
        if dacl.is_null() {
            problems.push("null dacl".to_string());
        } else {
            // SAFETY: `dacl` points into `sd`, alive until `_free` drops.
            let count = unsafe { (*dacl).AceCount };
            for i in 0..u32::from(count) {
                let mut ace: *mut c_void = null_mut();
                // SAFETY: `i` < AceCount.
                if unsafe { GetAce(dacl, i, &mut ace) } == 0 {
                    problems.push(format!("unreadable ace {i}"));
                    continue;
                }
                // SAFETY: every ACE starts with an ACE_HEADER.
                let header = unsafe { *(ace as *const ACE_HEADER) };
                match header.AceType {
                    ACCESS_DENIED_ACE_TYPE
                    | ACCESS_DENIED_CALLBACK_ACE_TYPE
                    | ACCESS_DENIED_OBJECT_ACE_TYPE
                    | ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE => {}
                    ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE => {
                        let allowed = ace as *const ACCESS_ALLOWED_ACE;
                        // SAFETY: both types store the SID at `SidStart`.
                        let sid = unsafe { std::ptr::addr_of!((*allowed).SidStart) } as PSID;
                        match sid_string(sid) {
                            Some(s) if s == user || ALLOWED.contains(&s.as_str()) => {}
                            Some(s) => problems.push(format!("allows {s}")),
                            None => problems.push(format!("unreadable ace {i}")),
                        }
                    }
                    other => problems.push(format!("unsupported ace {other}")),
                }
            }
        }
        Ok(PrivateDirReport {
            ok: problems.is_empty(),
            owner: owner_sid,
            problems,
        })
    }

    pub fn make(path: &str) -> Result<()> {
        let user = current_user(path)?;
        let sddl = wide(&format!(
            "O:{user}D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
        ));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: NUL-terminated SDDL; `sd` receives a LocalAlloc'ed descriptor.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        } == 0
        {
            return Err(io_error(
                path,
                "build a DACL for",
                &std::io::Error::last_os_error(),
            ));
        }
        let _free = Local(sd);
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl: *mut ACL = null_mut();
        // SAFETY: `sd` is a valid descriptor.
        if unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) } == 0
            || present == 0
        {
            return Err(io_error(
                path,
                "read the DACL built for",
                &std::io::Error::last_os_error(),
            ));
        }
        // The owner SID from the descriptor: `ConvertStringSidToSidW` would need its own free.
        let mut owner: PSID = null_mut();
        let mut owner_defaulted = 0;
        // SAFETY: `sd` is valid.
        unsafe {
            windows_sys::Win32::Security::GetSecurityDescriptorOwner(
                sd,
                &mut owner,
                &mut owner_defaulted,
            )
        };
        let name = wide(path);
        // SAFETY: valid name, owner SID and DACL (both inside `sd`).
        let rc = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                owner,
                null_mut(),
                dacl,
                null(),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win_error(path, "set the DACL of", rc));
        }
        Ok(())
    }
}
