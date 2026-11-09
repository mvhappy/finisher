//! Starting a process in its container (plan 7C Decisions 3 and 16): `CreateProcessW` with
//! `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` (the container SID and the plan's
//! capabilities), `PROC_THREAD_ATTRIBUTE_JOB_LIST` (the container's job, so nothing runs outside
//! it), `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` (the standard handles alone), the private desktop,
//! and an explicit environment block. The launcher stays the parent, relays the exit code and
//! reports what the job observed.

use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::Security::{SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, UpdateProcThreadAttribute,
};

use super::appcontainer;
use super::desktop::PrivateDesktop;
use super::job::ContainerJob;
use super::plan::ContainerPlan;
use super::sid::{Sid, wide};
use super::{check, owned};

/// `SE_GROUP_ENABLED` (winnt.h; windows-sys puts it behind `Win32_System_SystemServices`).
const SE_GROUP_ENABLED: u32 = 0x4;

/// A container's SID and the capabilities a process of it carries.
pub struct Container {
    pub name: String,
    pub sid: Sid,
    pub capabilities: Vec<Sid>,
}

impl Container {
    /// The plan's container (its profile created on first use) and capabilities.
    pub fn of(plan: &ContainerPlan) -> io::Result<Container> {
        Ok(Container {
            name: plan.container.clone(),
            sid: appcontainer::ensure(&plan.container)?,
            capabilities: plan
                .capabilities
                .iter()
                .map(|c| Sid::parse(&c.sid))
                .collect::<io::Result<_>>()?,
        })
    }

    /// `SECURITY_CAPABILITIES` for `UpdateProcThreadAttribute`, as one self-contained buffer:
    /// the struct, then its `SID_AND_ATTRIBUTES` array, then the SIDs, its pointers aimed inside
    /// the buffer. The heap block never moves, so the value stays valid while the buffer lives
    /// (the job host's `WinAttribute`); it must not be copied byte for byte into another one.
    pub fn security_capabilities(&self) -> io::Result<Box<[u8]>> {
        let n = self.capabilities.len();
        let head = size_of::<SECURITY_CAPABILITIES>();
        let array = n * size_of::<SID_AND_ATTRIBUTES>();
        let sids: Vec<Vec<u8>> = std::iter::once(&self.sid)
            .chain(&self.capabilities)
            .map(Sid::bytes)
            .collect();
        let total = head
            + array
            + sids
                .iter()
                .map(|s| s.len().next_multiple_of(8))
                .sum::<usize>();
        let mut buf = vec![0u8; total].into_boxed_slice();
        let base = buf.as_mut_ptr();
        if !(base as usize).is_multiple_of(align_of::<SECURITY_CAPABILITIES>()) {
            return Err(io::Error::other("the capabilities buffer is not aligned"));
        }
        let mut at = head + array;
        let mut ptrs = Vec::with_capacity(sids.len());
        for s in &sids {
            // SAFETY: `at + s.len()` is within `total` by construction.
            unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), base.add(at), s.len()) };
            // SAFETY: as above.
            ptrs.push(unsafe { base.add(at) }.cast::<c_void>());
            at += s.len().next_multiple_of(8);
        }
        // SAFETY: `head` and `array` bytes at the start of an aligned block are reserved for the
        // struct and the array; each write is in bounds and aligned.
        unsafe {
            let arr = base.add(head).cast::<SID_AND_ATTRIBUTES>();
            for (i, p) in ptrs[1..].iter().enumerate() {
                arr.add(i).write(SID_AND_ATTRIBUTES {
                    Sid: *p,
                    Attributes: SE_GROUP_ENABLED,
                });
            }
            base.cast::<SECURITY_CAPABILITIES>()
                .write(SECURITY_CAPABILITIES {
                    AppContainerSid: ptrs[0],
                    Capabilities: if n == 0 { null_mut() } else { arr },
                    CapabilityCount: n as u32,
                    Reserved: 0,
                });
        }
        Ok(buf)
    }
}

/// A `PROC_THREAD_ATTRIBUTE_LIST`; the values it points to must outlive its use.
struct AttrList(Vec<usize>);

impl AttrList {
    fn new(count: u32) -> io::Result<AttrList> {
        let mut size = 0usize;
        // SAFETY: a size query.
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size) };
        let mut buf = vec![0usize; size.div_ceil(size_of::<usize>()).max(1)];
        // SAFETY: `buf` is at least `size` bytes, pointer-aligned.
        check(unsafe {
            InitializeProcThreadAttributeList(buf.as_mut_ptr().cast(), count, 0, &mut size)
        })?;
        Ok(AttrList(buf))
    }

    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }

    fn set(&mut self, attribute: usize, value: *const c_void, len: usize) -> io::Result<()> {
        let list = self.ptr();
        // SAFETY: an initialised list with room; `value` is `len` bytes and outlives the list.
        check(unsafe {
            UpdateProcThreadAttribute(list, 0, attribute, value, len, null_mut(), null())
        })
    }
}

impl Drop for AttrList {
    fn drop(&mut self) {
        // SAFETY: an initialised list, deleted once.
        unsafe { DeleteProcThreadAttributeList(self.ptr()) };
    }
}

/// Quotes one argument as the MSVC runtime parses it.
fn quote(arg: &OsStr, out: &mut Vec<u16>) {
    let a: Vec<u16> = arg.encode_wide().collect();
    let plain = !a.is_empty()
        && !a
            .iter()
            .any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16);
    if plain {
        out.extend(&a);
        return;
    }
    out.push(b'"' as u16);
    let mut backslashes = 0;
    for &c in &a {
        if c == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        if c == b'"' as u16 {
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        }
        backslashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    out.push(b'"' as u16);
}

/// The command line of `argv` (NUL-terminated); an argument holding a NUL is refused.
pub fn command_line(argv: &[OsString]) -> io::Result<Vec<u16>> {
    let mut out = Vec::new();
    for (i, a) in argv.iter().enumerate() {
        if a.encode_wide().any(|c| c == 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an argument holds a NUL",
            ));
        }
        if i > 0 {
            out.push(b' ' as u16);
        }
        quote(a, &mut out);
    }
    out.push(0);
    Ok(out)
}

/// An environment block: `K=V\0…\0`, sorted without case as Windows expects; the last of two
/// names that differ only in case wins.
pub fn env_block(env: &[(OsString, OsString)]) -> io::Result<Vec<u16>> {
    let mut vars: Vec<(String, &OsString, &OsString)> = Vec::new();
    for (k, v) in env {
        let key = k.to_string_lossy().to_uppercase();
        if k.is_empty()
            || k.encode_wide().any(|c| c == 0 || c == b'=' as u16)
            || v.encode_wide().any(|c| c == 0)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("environment entry {k:?}"),
            ));
        }
        vars.retain(|(u, _, _)| *u != key);
        vars.push((key, k, v));
    }
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::new();
    for (_, k, v) in vars {
        out.extend(k.encode_wide());
        out.push(b'=' as u16);
        out.extend(v.encode_wide());
        out.push(0);
    }
    if out.is_empty() {
        out.push(0);
    }
    out.push(0);
    Ok(out)
}

/// This process's standard handles, made inheritable (for the handle list), without repeats.
fn std_handles() -> io::Result<([HANDLE; 3], Vec<HANDLE>)> {
    let mut std = [null_mut(); 3];
    let mut list = Vec::new();
    for (i, id) in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
        .into_iter()
        .enumerate()
    {
        // SAFETY: a query of this process's standard handle.
        let h = unsafe { GetStdHandle(id) };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            continue;
        }
        std[i] = h;
        if !list.contains(&h) {
            // SAFETY: a handle of this process; only its inherit flag changes.
            check(unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) })?;
            list.push(h);
        }
    }
    Ok((std, list))
}

/// A started container process.
pub struct Started {
    pub process: OwnedHandle,
    pub pid: u32,
}

/// Starts `argv` in `container`, inside `job`, on `desktop`, with exactly `env`, inheriting
/// this process's standard handles and nothing else. Error mode: the caller's (set it first,
/// [`super::wer::crash_ui_off`]; `CREATE_DEFAULT_ERROR_MODE` is never passed).
pub fn start(
    container: &Container,
    job: &ContainerJob,
    desktop: &PrivateDesktop,
    argv: &[OsString],
    env: &[(OsString, OsString)],
    cwd: Option<&Path>,
) -> io::Result<Started> {
    let caps = container.security_capabilities()?;
    let mut line = command_line(argv)?;
    let block = env_block(env)?;
    let cwd = cwd.map(|c| wide(c.as_os_str()));
    let desk = wide(desktop.name());
    let (std, handles) = std_handles()?;
    let jobs = [job.handle()];
    let mut list = AttrList::new(3)?;
    list.set(
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
        caps.as_ptr().cast(),
        size_of::<SECURITY_CAPABILITIES>(),
    )?;
    list.set(
        PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
        jobs.as_ptr().cast(),
        size_of::<HANDLE>(),
    )?;
    if !handles.is_empty() {
        list.set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            handles.len() * size_of::<HANDLE>(),
        )?;
    }
    // SAFETY: a plain C struct; zeros are its defaults.
    let mut si: STARTUPINFOEXW = unsafe { zeroed() };
    si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = std[0];
    si.StartupInfo.hStdOutput = std[1];
    si.StartupInfo.hStdError = std[2];
    si.StartupInfo.lpDesktop = desk.as_ptr().cast_mut();
    si.lpAttributeList = list.ptr();
    // SAFETY: a plain C struct, filled by the call.
    let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
    // SAFETY: every pointer is valid and NUL-terminated where it must be; `line` is writable;
    // the attribute list and everything it points to outlive the call.
    check(unsafe {
        CreateProcessW(
            null(),
            line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            block.as_ptr().cast(),
            cwd.as_ref().map_or(null(), |c| c.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        )
    })?;
    drop(owned(pi.hThread)?);
    Ok(Started {
        process: owned(pi.hProcess)?,
        pid: pi.dwProcessId,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(args: &[&str]) -> String {
        let v: Vec<OsString> = args.iter().map(OsString::from).collect();
        let w = command_line(&v).unwrap();
        String::from_utf16(&w[..w.len() - 1]).unwrap()
    }

    #[test]
    fn arguments_are_quoted_as_msvcrt_parses_them() {
        assert_eq!(line(&["a", "b c", ""]), r#"a "b c" """#);
        assert_eq!(line(&[r#"x"y"#, r"c:\p\"]), r#""x\"y" c:\p\"#);
        assert_eq!(line(&[r"c:\a b\"]), r#""c:\a b\\""#);
        assert!(command_line(&[OsString::from("a\0b")]).is_err());
    }

    #[test]
    fn env_block_is_sorted_and_deduplicated_without_case() {
        let e = |k: &str, v: &str| (OsString::from(k), OsString::from(v));
        let b = env_block(&[e("b", "1"), e("A", "2"), e("B", "3")]).unwrap();
        assert_eq!(String::from_utf16(&b).unwrap(), "A=2\0B=3\0\0");
        assert!(env_block(&[e("a=b", "1")]).is_err());
    }
}
