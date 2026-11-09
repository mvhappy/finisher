//! A process's own DACL (plan 7C Task 9 review B1). An AppContainer token's default DACL
//! admits its container SID with full access, so every process of a container could open
//! every other one (`PROCESS_ALL_ACCESS`, verified on the S9 host). Processes of one
//! container belong to different sessions, hooks, jobs or calls, so the sandbox gives each
//! process an explicit descriptor at creation and, while it is still suspended, sets its
//! token's default DACL to the same entries, so the threads and processes it creates later
//! get them too. The entries name `SYSTEM`, the user and the process's own capability (its
//! session's, hook's, job's or call's): processes of one session may still open each other.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    ACL, GetSecurityDescriptorDacl, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_DEFAULT_DACL, TOKEN_QUERY, TokenDefaultDacl,
};
use windows_sys::Win32::System::Threading::OpenProcessToken;

/// The SDDL of a sandboxed process's DACL: `SYSTEM`, `user` and `own` (a capability SID).
pub fn sddl(user: &str, own: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;GA;;;{user})(A;;GA;;;{own})")
}

/// A self-relative descriptor parsed from SDDL (freed on drop).
pub struct Descriptor(PSECURITY_DESCRIPTOR);

// SAFETY: plain memory owned by this value alone, read-only after creation.
unsafe impl Send for Descriptor {}
// SAFETY: as above.
unsafe impl Sync for Descriptor {}

impl Descriptor {
    pub fn parse(sddl: &str) -> io::Result<Descriptor> {
        let w: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: a NUL-terminated string and a valid out-pointer.
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
        Ok(Descriptor(sd))
    }

    /// `SECURITY_ATTRIBUTES` for `CreateProcess*`'s process and thread arguments; valid while
    /// `self` lives.
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }

    /// Makes the descriptor's DACL the default DACL of `process`'s primary token (call it
    /// before the process's first thread runs).
    pub fn set_default_dacl(&self, process: &impl AsRawHandle) -> io::Result<()> {
        let process = process.as_raw_handle() as HANDLE;
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = null_mut();
        // SAFETY: our descriptor and valid out-pointers.
        if unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) }
            == 0
            || present == 0
        {
            return Err(io::Error::other("the descriptor has no DACL"));
        }
        let mut tok: HANDLE = null_mut();
        // SAFETY: a process handle the caller owns and a valid out-pointer.
        if unsafe { OpenProcessToken(process, TOKEN_ADJUST_DEFAULT | TOKEN_QUERY, &mut tok) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh handle we own.
        let tok = unsafe { OwnedHandle::from_raw_handle(tok as _) };
        let info = TOKEN_DEFAULT_DACL { DefaultDacl: dacl };
        // SAFETY: our token with TOKEN_ADJUST_DEFAULT and a value of the class's type.
        let ok = unsafe {
            SetTokenInformation(
                tok.as_raw_handle() as HANDLE,
                TokenDefaultDacl,
                (&raw const info).cast(),
                size_of::<TOKEN_DEFAULT_DACL>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: LocalAlloc'd by the conversion, freed once.
        unsafe { LocalFree(self.0 as HLOCAL) };
    }
}
