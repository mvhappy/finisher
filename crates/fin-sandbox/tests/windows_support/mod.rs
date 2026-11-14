//! Shared by the Windows suites (plan 7C Task 9): an owner-only tree, the launcher run with a
//! request on an inherited pipe, and the effective access of a container token computed with
//! Windows' own `AccessCheck` (owner decision 2026-11-08: the suites never try to reach what a
//! container was not granted; they compute that it cannot).

#![allow(dead_code)]

use std::ffi::c_void;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::ptr::null_mut;

use fin_sandbox::launch::LaunchRequest;
use fin_sandbox::windows::acl;
use fin_sandbox::windows::sid::{Sid, current_user, wide};
use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation,
};
use windows_sys::Win32::Security::{
    AccessCheck, DACL_SECURITY_INFORMATION, DuplicateTokenEx, GENERIC_MAPPING,
    GROUP_SECURITY_INFORMATION, GetFileSecurityW, LABEL_SECURITY_INFORMATION, MapGenericMask,
    OWNER_SECURITY_INFORMATION, PRIVILEGE_SET, PSECURITY_DESCRIPTOR, SecurityImpersonation,
    TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY, TokenImpersonation,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

pub const EXEC: &str = env!("CARGO_BIN_EXE_fin-sandbox-exec");
pub const PROBE: &str = env!("CARGO_BIN_EXE_fin-sandbox-probe");

/// The file generic mapping (`FILE_GENERIC_*`, `FILE_ALL_ACCESS`).
pub const FILE_MAPPING: GENERIC_MAPPING = GENERIC_MAPPING {
    GenericRead: 0x0012_0089,
    GenericWrite: 0x0012_0116,
    GenericExecute: 0x0012_00A0,
    GenericAll: 0x001F_01FF,
};
/// `MAXIMUM_ALLOWED`: what `AccessCheck` grants, whatever it is.
pub const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
/// `FILE_READ_DATA` / `FILE_LIST_DIRECTORY`.
pub const FILE_READ_DATA: u32 = 0x1;
/// `FILE_WRITE_DATA` / `FILE_ADD_FILE`.
pub const FILE_WRITE_DATA: u32 = 0x2;

/// A temporary root with `private_dir`'s protected owner-only DACL, deleted on drop.
pub struct Tree {
    _d: tempfile::TempDir,
    pub t: PathBuf,
}

pub fn owner_only_sddl() -> String {
    let user = current_user().unwrap().string();
    format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)")
}

impl Tree {
    pub fn new() -> Tree {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().to_path_buf();
        acl::set_sddl(&t, &owner_only_sddl()).unwrap();
        Tree { _d: d, t }
    }

    /// `rel` (written with `/`) under the root, in Windows' own form.
    pub fn p(&self, rel: &str) -> PathBuf {
        self.t.join(rel.replace('/', "\\"))
    }

    pub fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.p(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    pub fn dir(&self, rel: &str) -> PathBuf {
        let p = self.p(rel);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

/// Grants `name`'s capability SID (or a SID string) `mask` on `path`, inheritable, as the
/// broker does.
pub fn grant(path: &Path, sid: &str, mask: u32) {
    acl::grant(path, &Sid::parse(sid).unwrap(), mask, true).unwrap();
}

/// The launcher started under `req` with `argv`, its stdin, stdout and stderr piped. The
/// request travels on an inherited pipe whose handle value is `--policy-fd`.
pub fn spawn(req: &LaunchRequest, argv: &[&str], env: &[(&str, &str)]) -> Child {
    spawn_in(req, argv, env, None)
}

/// [`spawn`] in `cwd`.
pub fn spawn_in(
    req: &LaunchRequest,
    argv: &[&str],
    env: &[(&str, &str)],
    cwd: Option<&Path>,
) -> Child {
    let bytes = serde_json::to_vec(req).unwrap();
    let (r, mut w) = std::io::pipe().unwrap();
    let writer = std::thread::spawn(move || {
        w.write_all(&bytes).unwrap();
    });
    let h = r.as_raw_handle() as HANDLE;
    // SAFETY: a pipe end we own; only its inherit flag changes.
    assert_ne!(
        unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) },
        0
    );
    let fd = (h as usize).to_string();
    let mut cmd = Command::new(EXEC);
    // `{fd}` in `argv` is the request handle's value (to check it does not reach the program).
    cmd.args(["--policy-fd", &fd, "--"])
        .args(argv.iter().map(|a| a.replace("{fd}", &fd)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    let child = cmd.spawn().unwrap();
    drop(r);
    writer.join().unwrap();
    child
}

/// What one launch showed.
#[derive(Debug)]
pub struct Run {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    /// The probe's JSON line (the last line on stdout).
    pub fn json(&self) -> serde_json::Value {
        let line = self
            .stdout
            .lines()
            .last()
            .unwrap_or_else(|| panic!("no output: {self:?}"));
        serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {self:?}"))
    }
}

pub fn run(req: &LaunchRequest, argv: &[&str]) -> Run {
    run_env(req, argv, &[])
}

pub fn run_env(req: &LaunchRequest, argv: &[&str], env: &[(&str, &str)]) -> Run {
    run_in(req, argv, env, None)
}

/// [`run_env`] in `cwd`.
pub fn run_in(req: &LaunchRequest, argv: &[&str], env: &[(&str, &str)], cwd: Option<&Path>) -> Run {
    let child = spawn_in(req, argv, env, cwd);
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The probe through the launcher: `fin-sandbox-probe <op…>`.
pub fn probe(req: &LaunchRequest, op: &[&str]) -> serde_json::Value {
    let mut argv = vec![PROBE];
    argv.extend(op);
    let r = run(req, &argv);
    assert_eq!(r.code, Some(0), "{r:?}");
    r.json()
}

/// A probe waiting in its container (`wait`): its pid and token line; it ends when dropped.
pub struct Waiting {
    pub child: Child,
    pub token: serde_json::Value,
}

impl Waiting {
    pub fn start(req: &LaunchRequest) -> Waiting {
        Waiting::start_argv(req, &[PROBE, "wait"])
    }

    /// A waiting probe started by `argv` (its first stdout line is the waiting one's token).
    pub fn start_argv(req: &LaunchRequest, argv: &[&str]) -> Waiting {
        let mut child = spawn(req, argv, &[]);
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        let token: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(t) => t,
            Err(e) => {
                let _ = child.kill();
                let out = child.wait_with_output().unwrap();
                panic!("{e}: {line:?} {}", String::from_utf8_lossy(&out.stderr))
            }
        };
        Waiting { child, token }
    }

    pub fn pid(&self) -> u32 {
        self.token["pid"].as_u64().expect("a pid") as u32
    }

    /// An impersonation copy of the container process's token.
    pub fn token_handle(&self) -> OwnedHandle {
        impersonation_token(self.pid())
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

fn owned(h: HANDLE) -> OwnedHandle {
    assert!(!h.is_null(), "{}", std::io::Error::last_os_error());
    // SAFETY: a fresh handle we own.
    unsafe { OwnedHandle::from_raw_handle(h as _) }
}

/// An impersonation token duplicated from process `pid`'s primary token.
pub fn impersonation_token(pid: u32) -> OwnedHandle {
    // SAFETY: plain handle calls with valid out-pointers; each handle is owned once.
    unsafe {
        let p = owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid));
        let mut tok: HANDLE = null_mut();
        assert_ne!(
            OpenProcessToken(
                p.as_raw_handle() as HANDLE,
                TOKEN_DUPLICATE | TOKEN_QUERY,
                &mut tok
            ),
            0,
            "OpenProcessToken: {}",
            std::io::Error::last_os_error()
        );
        let tok = owned(tok);
        let mut dup: HANDLE = null_mut();
        assert_ne!(
            DuplicateTokenEx(
                tok.as_raw_handle() as HANDLE,
                TOKEN_QUERY | TOKEN_IMPERSONATE,
                std::ptr::null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut dup,
            ),
            0,
            "DuplicateTokenEx: {}",
            std::io::Error::last_os_error()
        );
        owned(dup)
    }
}

/// The access `token` is granted by `sd` for `MAXIMUM_ALLOWED` (0: none).
///
/// # Safety
/// `sd` is a valid self-relative security descriptor with owner, group and DACL.
pub unsafe fn access_of(token: &OwnedHandle, sd: PSECURITY_DESCRIPTOR) -> u32 {
    // SAFETY: the caller's promise.
    unsafe { access_with(token, sd, FILE_MAPPING) }
}

/// The process generic mapping (`PROCESS_*`).
pub const PROCESS_MAPPING: GENERIC_MAPPING = GENERIC_MAPPING {
    GenericRead: 0x0002_0410,
    GenericWrite: 0x0002_0BEA,
    GenericExecute: 0x0012_1000,
    GenericAll: 0x001F_FFFF,
};

/// [`access_of`] with another generic mapping.
///
/// # Safety
/// As [`access_of`].
pub unsafe fn access_with(
    token: &OwnedHandle,
    sd: PSECURITY_DESCRIPTOR,
    mapping: GENERIC_MAPPING,
) -> u32 {
    let mut desired = MAXIMUM_ALLOWED;
    // SAFETY: valid pointers.
    unsafe { MapGenericMask(&mut desired, &mapping) };
    let mut privs = vec![0u64; 32];
    let mut privs_len = (privs.len() * 8) as u32;
    let (mut granted, mut status) = (0u32, 0i32);
    // SAFETY: the caller's descriptor, our token, buffers of the sizes given.
    let ok = unsafe {
        AccessCheck(
            sd,
            token.as_raw_handle() as HANDLE,
            desired,
            &mapping,
            privs.as_mut_ptr().cast::<PRIVILEGE_SET>(),
            &mut privs_len,
            &mut granted,
            &mut status,
        )
    };
    assert_ne!(ok, 0, "AccessCheck: {}", std::io::Error::last_os_error());
    if status == 0 { 0 } else { granted }
}

/// The access `token` has on `path` by its security descriptor (owner, group, DACL, label).
pub fn file_access(token: &OwnedHandle, path: &Path) -> u32 {
    let si = OWNER_SECURITY_INFORMATION
        | GROUP_SECURITY_INFORMATION
        | DACL_SECURITY_INFORMATION
        | LABEL_SECURITY_INFORMATION;
    let p = wide(path);
    let mut len = 0u32;
    // SAFETY: a size query.
    unsafe { GetFileSecurityW(p.as_ptr(), si, null_mut(), 0, &mut len) };
    assert!(
        len > 0,
        "{}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: a buffer of `len` bytes.
    let ok = unsafe {
        GetFileSecurityW(
            p.as_ptr(),
            si,
            buf.as_mut_ptr().cast::<c_void>(),
            len,
            &mut len,
        )
    };
    assert_ne!(
        ok,
        0,
        "{}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
    // SAFETY: the descriptor just read.
    unsafe { access_of(token, buf.as_mut_ptr().cast()) }
}

/// A named pipe server with the descriptor of `sddl`, its server handle.
pub fn pipe_server(name: &str, sddl: &str) -> OwnedHandle {
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: a NUL-terminated string and a valid out-pointer (leaked: test only).
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(sddl).as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        },
        0
    );
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    // SAFETY: a NUL-terminated name and valid attributes.
    let h = unsafe {
        CreateNamedPipeW(
            wide(name).as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            4096,
            0,
            &sa,
        )
    };
    assert_ne!(
        h,
        windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE,
        "{}",
        std::io::Error::last_os_error()
    );
    owned(h)
}

/// The access `token` has on a kernel object by its security descriptor.
pub fn object_access(token: &OwnedHandle, object: &OwnedHandle) -> u32 {
    object_access_with(token, object, FILE_MAPPING)
}

/// The access `token` has on process `pid` by its security descriptor.
pub fn process_access(token: &OwnedHandle, pid: u32) -> u32 {
    const READ_CONTROL: u32 = 0x0002_0000;
    // SAFETY: a handle we own.
    let p = owned(unsafe { OpenProcess(READ_CONTROL, 0, pid) });
    object_access_with(token, &p, PROCESS_MAPPING)
}

/// [`object_access`] with another generic mapping.
pub fn object_access_with(
    token: &OwnedHandle,
    object: &OwnedHandle,
    mapping: GENERIC_MAPPING,
) -> u32 {
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    let si = OWNER_SECURITY_INFORMATION
        | GROUP_SECURITY_INFORMATION
        | DACL_SECURITY_INFORMATION
        | LABEL_SECURITY_INFORMATION;
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: our handle and a valid out-pointer (leaked: test only).
    let e = unsafe {
        GetSecurityInfo(
            object.as_raw_handle() as HANDLE,
            SE_KERNEL_OBJECT,
            si,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    };
    assert_eq!(e, 0, "GetSecurityInfo: {e}");
    // SAFETY: the descriptor just read.
    unsafe { access_with(token, sd, mapping) }
}

/// Whether process `pid` ends within `ms`.
pub fn ends_within(pid: u32, ms: u32) -> bool {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a handle we close; a bounded wait.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return true; // already gone
        }
        let w = WaitForSingleObject(h, ms);
        CloseHandle(h);
        w == WAIT_OBJECT_0
    }
}

/// Watches for `secs` seconds what a crash leaves: every `WerFault.exe` (`werfault: <pid>`), one
/// with a window (`window: <pid> <title>`), and any report directory new in the per-user or
/// machine `ReportArchive`/`ReportQueue` (`report: <path>`; WER shortens application names in
/// them, so none is filtered by name). `MainWindowHandle` sees the watcher's own desktop.
pub fn wer_watch(secs: u32) -> Child {
    let script = format!(
        "$roots=@(\"$env:LOCALAPPDATA\\Microsoft\\Windows\\WER\\ReportArchive\", \
                  \"$env:LOCALAPPDATA\\Microsoft\\Windows\\WER\\ReportQueue\", \
                  \"$env:ProgramData\\Microsoft\\Windows\\WER\\ReportArchive\", \
                  \"$env:ProgramData\\Microsoft\\Windows\\WER\\ReportQueue\"); \
         $before=@{{}}; foreach($r in $roots){{ Get-ChildItem -LiteralPath $r -Directory -ErrorAction SilentlyContinue | ForEach-Object {{ $before[$_.FullName]=1 }} }}; \
         $old=@{{}}; Get-Process -Name WerFault -ErrorAction SilentlyContinue | ForEach-Object {{ $old[$_.Id]=1 }}; \
         $end=(Get-Date).AddSeconds({secs}); $hit=@{{}}; \
         while((Get-Date) -lt $end) {{ \
           Get-Process -Name WerFault -ErrorAction SilentlyContinue | Where-Object {{ -not $old.ContainsKey($_.Id) }} | ForEach-Object {{ \
             $hit[\"werfault: $($_.Id)\"]=1; \
             if ($_.MainWindowHandle -ne 0) {{ $hit[\"window: $($_.Id) $($_.MainWindowTitle)\"]=1 }} }}; \
           Start-Sleep -Milliseconds 50 }}; \
         foreach($r in $roots){{ Get-ChildItem -LiteralPath $r -Directory -ErrorAction SilentlyContinue | \
           Where-Object {{ -not $before.ContainsKey($_.FullName) }} | \
           ForEach-Object {{ $hit[\"report: $($_.FullName)\"]=1 }} }}; \
         $hit.Keys"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("powershell runs")
}
