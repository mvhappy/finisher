//! Windows helpers for `tests/pipe_server.rs`: client ends opened as `fin` opens them, the pipe's
//! information and DACL read back, a token whose user SID is deny-only, and a throwaway
//! AppContainer that runs this test binary's `appcontainer_child`.

use std::fs::File;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

pub use windows_sys::Wdk::Storage::FileSystem::{
    FILE_PIPE_LOCAL_INFORMATION, FILE_PIPE_REJECT_REMOTE_CLIENTS,
};
use windows_sys::Wdk::Storage::FileSystem::{FilePipeLocalInformation, NtQueryInformationFile};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, DACL_SECURITY_INFORMATION, FreeSid, ImpersonateLoggedOnUser,
    PSECURITY_DESCRIPTOR, PSID, RevertToSelf, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
    TOKEN_ALL_ACCESS,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_WRITE_DATA, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, WaitNamedPipeW};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, STARTUPINFOEXW,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

/// What `fin` asks for (`crates/fin/src/pipe_client.rs`).
pub const CLIENT_ACCESS: u32 = GENERIC_READ | FILE_WRITE_DATA;
pub const READ_CONTROL: u32 = 0x0002_0000;

pub const CHILD_CONNECTED: u32 = 40;
pub const CHILD_DENIED: u32 = 41;
pub const CHILD_OTHER: u32 = 42;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The client end of `name` with `access`, identification level (no waiting).
pub fn open(name: &str, access: u32) -> io::Result<File> {
    let w = wide(name);
    // SAFETY: `w` is NUL-terminated.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            access,
            0,
            null(),
            OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a new handle only this `File` owns.
    Ok(unsafe { File::from_raw_handle(h) })
}

/// [`open`], waiting up to `wait` while every instance is busy.
pub fn open_waiting(name: &str, wait: Duration) -> io::Result<File> {
    let until = Instant::now() + wait;
    loop {
        match open(name, CLIENT_ACCESS) {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(e);
                }
                // SAFETY: as above.
                unsafe { WaitNamedPipeW(wide(name).as_ptr(), left.as_millis().max(1) as u32) };
            }
            other => return other,
        }
    }
}

/// A plain instance of `name` with the default descriptor (a squatter).
pub fn raw_instance(name: &str) -> io::Result<OwnedHandle> {
    let w = wide(name);
    // SAFETY: `w` is NUL-terminated; default security.
    let h = unsafe { CreateNamedPipeW(w.as_ptr(), 3, 0, 1, 4096, 4096, 0, null()) };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a new handle only this value owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

pub fn pipe_info(f: &File) -> io::Result<FILE_PIPE_LOCAL_INFORMATION> {
    // SAFETY: zeroed out-structures; a live pipe handle.
    let mut io: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut info = FILE_PIPE_LOCAL_INFORMATION::default();
    let status = unsafe {
        NtQueryInformationFile(
            f.as_raw_handle() as HANDLE,
            &mut io,
            (&mut info as *mut FILE_PIPE_LOCAL_INFORMATION).cast(),
            size_of::<FILE_PIPE_LOCAL_INFORMATION>() as u32,
            FilePipeLocalInformation,
        )
    };
    if status < 0 {
        return Err(io::Error::other(format!("NTSTATUS {status:#x}")));
    }
    Ok(info)
}

/// The pipe's DACL as SDDL, through a client handle opened with `READ_CONTROL`.
pub fn dacl_sddl(f: &File) -> io::Result<String> {
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: a live handle; `sd` receives a LocalAlloc'ed descriptor.
    let rc = unsafe {
        GetSecurityInfo(
            f.as_raw_handle() as HANDLE,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    let mut out: *mut u16 = null_mut();
    // SAFETY: `sd` is valid; `out` receives a LocalAlloc'ed string.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut out,
            null_mut(),
        )
    };
    let result = if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        let mut len = 0;
        // SAFETY: NUL-terminated.
        while unsafe { *out.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len` u16s were just read.
        Ok(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(out, len)
        }))
    };
    // SAFETY: both were LocalAlloc'ed by the calls above.
    unsafe {
        LocalFree(sd);
        LocalFree(out.cast());
    }
    result
}

/// The trustee of each ACE in an SDDL string (`(A;;FA;;;S-1-…)` → `S-1-…`).
pub fn ace_sids(sddl: &str) -> Vec<String> {
    sddl.split('(')
        .skip(1)
        .filter_map(|ace| {
            ace.trim_end_matches(')')
                .split(';')
                .nth(5)
                .map(|s| s.trim_end_matches(')').to_string())
        })
        .collect()
}

/// Runs `f` while this thread impersonates this user with the user SID deny-only.
pub fn with_restricted_token<T>(f: impl FnOnce() -> T) -> T {
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser};
    let mut token: HANDLE = null_mut();
    let mut restricted: HANDLE = null_mut();
    // SAFETY: this process's token; the user SID read from it stays in `user` while used; both
    // handles are closed below.
    unsafe {
        assert_ne!(
            OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut token),
            0
        );
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
        let mut user = vec![0u64; (len as usize).div_ceil(8)];
        assert_ne!(
            GetTokenInformation(token, TokenUser, user.as_mut_ptr().cast(), len, &mut len),
            0
        );
        let disable = SID_AND_ATTRIBUTES {
            Sid: *user.as_ptr().cast::<PSID>(),
            Attributes: 0,
        };
        assert_ne!(
            CreateRestrictedToken(token, 0, 1, &disable, 0, null(), 0, null(), &mut restricted),
            0,
            "CreateRestrictedToken: {}",
            io::Error::last_os_error()
        );
        assert_ne!(ImpersonateLoggedOnUser(restricted), 0);
        let out = f();
        RevertToSelf();
        CloseHandle(restricted);
        CloseHandle(token);
        out
    }
}

/// A throwaway AppContainer and a copy of this test binary it may run.
pub struct AppContainer {
    name: String,
    pub sid: String,
    sid_ptr: PSID,
    dir: PathBuf,
    exe: PathBuf,
}

impl AppContainer {
    /// `None` when the host cannot make one (profiles disabled, no access).
    pub fn create() -> Option<AppContainer> {
        let name = format!("finisher.pipe-test.{}", std::process::id());
        let w = wide(&name);
        let mut sid: PSID = null_mut();
        // SAFETY: NUL-terminated names; `sid` receives a SID freed in `Drop`.
        let hr = unsafe {
            CreateAppContainerProfile(w.as_ptr(), w.as_ptr(), w.as_ptr(), null(), 0, &mut sid)
        };
        if hr < 0 {
            // SAFETY: as above.
            if unsafe { DeriveAppContainerSidFromAppContainerName(w.as_ptr(), &mut sid) } < 0 {
                return None;
            }
        }
        let sid_str = crate::pipe_sec::sid_string(sid).ok()?;
        // A directory the container may read and run from (ALL APPLICATION PACKAGES: RX).
        let dir = std::env::temp_dir().join(format!("fin-pipe-ac-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let exe = dir.join("pipe_server_child.exe");
        std::fs::copy(std::env::current_exe().ok()?, &exe).ok()?;
        let granted = std::process::Command::new("icacls")
            .arg(&dir)
            .args(["/grant", "*S-1-15-2-1:(OI)(CI)RX", "/T", "/Q"])
            .output()
            .ok()?;
        if !granted.status.success() {
            return None;
        }
        Some(AppContainer {
            name,
            sid: sid_str,
            sid_ptr: sid,
            dir,
            exe,
        })
    }

    /// Runs `appcontainer_child` in the container against `pipe`; its exit code.
    pub fn probe(&self, pipe: &str) -> u32 {
        // SAFETY: the attribute list is sized by the first call and freed below; the
        // capabilities and the SID outlive `CreateProcessW`.
        unsafe {
            let mut caps: SECURITY_CAPABILITIES = zeroed();
            caps.AppContainerSid = self.sid_ptr;
            let mut size = 0usize;
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
            let mut attrs = vec![0u8; size];
            let list = attrs.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            assert_ne!(InitializeProcThreadAttributeList(list, 1, 0, &mut size), 0);
            assert_ne!(
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                    (&caps as *const SECURITY_CAPABILITIES).cast(),
                    size_of::<SECURITY_CAPABILITIES>(),
                    null_mut(),
                    null(),
                ),
                0
            );
            let mut si: STARTUPINFOEXW = zeroed();
            si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = list;
            let mut pi: PROCESS_INFORMATION = zeroed();
            let exe = self.exe.to_string_lossy().to_string();
            let mut cmd = wide(&format!(
                "\"{exe}\" appcontainer_child --exact --ignored --test-threads=1"
            ));
            let mut env: Vec<u16> = Vec::new();
            for (k, v) in std::env::vars().filter(|(k, _)| k != "FIN_PIPE_PROBE") {
                env.extend(format!("{k}={v}").encode_utf16());
                env.push(0);
            }
            env.extend(format!("FIN_PIPE_PROBE={pipe}").encode_utf16());
            env.extend([0, 0]);
            let dir = wide(&self.dir.to_string_lossy());
            let ok = CreateProcessW(
                null(),
                cmd.as_mut_ptr(),
                null(),
                null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                env.as_ptr().cast(),
                dir.as_ptr(),
                &si.StartupInfo,
                &mut pi,
            );
            DeleteProcThreadAttributeList(list);
            assert_ne!(
                ok,
                0,
                "CreateProcessW in the AppContainer: {}",
                io::Error::last_os_error()
            );
            assert_eq!(
                WaitForSingleObject(pi.hProcess, 30_000),
                WAIT_OBJECT_0,
                "the child finished"
            );
            let mut code = 0u32;
            GetExitCodeProcess(pi.hProcess, &mut code);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            code
        }
    }
}

impl Drop for AppContainer {
    fn drop(&mut self) {
        let w = wide(&self.name);
        // SAFETY: the profile and SID this value made.
        unsafe {
            DeleteAppContainerProfile(w.as_ptr());
            FreeSid(self.sid_ptr);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
