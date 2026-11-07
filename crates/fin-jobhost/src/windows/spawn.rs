//! Windows spawning (Decisions 10, 14, 21). Each process is made with `CreateProcessW` (or
//! `CreateProcessAsUserW` with a spawner's token) and
//! `CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT |
//! EXTENDED_STARTUPINFO_PRESENT` plus its QoS priority class, an explicit environment block,
//! the host's broker as its parent (`PROC_THREAD_ATTRIBUTE_PARENT_PROCESS`, `broker.rs`) and
//! `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` naming its three pipe ends, copied into the broker for
//! the call only. Without a broker there is no spawn (`noBroker`); an outer Job's
//! active-process limit refuses it as `processLimit`. The suspended process is then placed in
//! its Jobs (host → group →
//! leaf) and only then resumed: none of its code runs outside them. A failure at any step
//! after the creation terminates the suspended process before the error is returned.

use std::ffi::c_void;
use std::io::{self, PipeReader, PipeWriter};
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{ERROR_NOT_ENOUGH_QUOTA, HANDLE};
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    CreateProcessAsUserW, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    NORMAL_PRIORITY_CLASS, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_PARENT_PROCESS,
    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::broker::InBroker;
use super::env_block::{command_line, env_block, wide};
use super::{KILLED_EXIT_CODE, OsProc, SpawnCtx, Spawned, check, owned, raw};
use crate::domain::Placement;
use crate::spawner::{PlatformCommand, Qos};

/// The child's ends of its three pipes.
pub(crate) struct ChildStdio<'a> {
    pub stdin: &'a PipeReader,
    pub stdout: &'a PipeWriter,
    pub stderr: &'a PipeWriter,
}

/// A created, not yet resumed process. Dropped unresumed, it is terminated.
pub(crate) struct SuspendedProcess {
    process: Option<OwnedHandle>,
    thread: OwnedHandle,
    pub pid: u32,
}

impl SuspendedProcess {
    fn placement(&self) -> Placement {
        Placement {
            process: self.process.as_ref().expect("unresumed").as_raw_handle(),
            thread: self.thread.as_raw_handle(),
            pid: self.pid,
        }
    }

    /// Lets it run; its process handle is the waiter's.
    pub fn resume(mut self) -> io::Result<OwnedHandle> {
        // SAFETY: the primary thread of our suspended process.
        if unsafe { ResumeThread(raw(&self.thread)) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(self.process.take().expect("unresumed"))
    }
}

impl Drop for SuspendedProcess {
    fn drop(&mut self) {
        if let Some(p) = &self.process {
            // SAFETY: our own process handle, with PROCESS_TERMINATE and SYNCHRONIZE access.
            unsafe {
                TerminateProcess(raw(p), KILLED_EXIT_CODE);
                WaitForSingleObject(raw(p), 5_000);
            }
        }
    }
}

fn priority(qos: Qos) -> PROCESS_CREATION_FLAGS {
    match qos {
        Qos::Normal => NORMAL_PRIORITY_CLASS,
        Qos::Utility | Qos::Background => BELOW_NORMAL_PRIORITY_CLASS,
    }
}

/// A `PROC_THREAD_ATTRIBUTE_LIST`. The values it points to must outlive its use.
struct AttrList {
    buf: Vec<usize>,
}

impl AttrList {
    fn new(count: u32) -> io::Result<AttrList> {
        let mut size = 0usize;
        // SAFETY: a size query (it fails with ERROR_INSUFFICIENT_BUFFER and sets `size`).
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), count, 0, &mut size) };
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: `buf` is at least `size` bytes, pointer-aligned.
        check(unsafe {
            InitializeProcThreadAttributeList(buf.as_mut_ptr().cast(), count, 0, &mut size)
        })?;
        Ok(AttrList { buf })
    }

    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buf.as_mut_ptr().cast()
    }

    /// Adds one attribute; `value` must stay valid until the list is used.
    fn set(&mut self, attribute: usize, value: *const c_void, len: usize) -> io::Result<()> {
        let list = self.ptr();
        // SAFETY: an initialised list with room for this attribute; `value` is `len` bytes.
        check(unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                attribute,
                value,
                len,
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        })
    }
}

impl Drop for AttrList {
    fn drop(&mut self) {
        // SAFETY: an initialised list, deleted once.
        unsafe { DeleteProcThreadAttributeList(self.ptr()) };
    }
}

/// The reason a spawn an outer Job's active-process limit refused is reported with.
pub(crate) const PROCESS_LIMIT: &str = "processLimit";

/// Creates `cmd` suspended with `stdio` as its only inherited handles: copies in `broker`,
/// its parent, inheritable there for the call only.
pub(crate) fn spawn_suspended(
    cmd: &PlatformCommand,
    stdio: ChildStdio<'_>,
    flags: PROCESS_CREATION_FLAGS,
    broker: HANDLE,
) -> io::Result<SuspendedProcess> {
    let handle_list = PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize;
    let parent_attr = PROC_THREAD_ATTRIBUTE_PARENT_PROCESS as usize;
    if cmd
        .attributes
        .iter()
        .any(|a| a.attribute == handle_list || a.attribute == parent_attr)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a spawner may not set PROC_THREAD_ATTRIBUTE_HANDLE_LIST or _PARENT_PROCESS",
        ));
    }
    let app = wide(cmd.program.as_os_str())?;
    let mut line = command_line(&cmd.program, &cmd.args)?;
    let env = env_block(&cmd.env)?;
    let cwd = wide(cmd.cwd.as_os_str())?;
    let ours: [HANDLE; 3] = [raw(stdio.stdin), raw(stdio.stdout), raw(stdio.stderr)];
    // Dropped after the call: the broker's copies close.
    let in_broker = InBroker::new(broker, &ours)?;
    let handles: [HANDLE; 3] = [
        in_broker.handles[0],
        in_broker.handles[1],
        in_broker.handles[2],
    ];
    let mut list = AttrList::new(2 + cmd.attributes.len() as u32)?;
    list.set(parent_attr, (&raw const broker).cast(), size_of::<HANDLE>())?;
    list.set(
        handle_list,
        handles.as_ptr().cast(),
        size_of::<[HANDLE; 3]>(),
    )?;
    for a in &cmd.attributes {
        list.set(a.attribute, a.value.as_ptr().cast(), a.value.len())?;
    }
    // SAFETY: a plain C struct; zeros are its defaults.
    let mut si: STARTUPINFOEXW = unsafe { zeroed() };
    si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = handles[0];
    si.StartupInfo.hStdOutput = handles[1];
    si.StartupInfo.hStdError = handles[2];
    si.lpAttributeList = list.ptr();
    let flags = flags
        | CREATE_SUSPENDED
        | CREATE_NO_WINDOW
        | CREATE_UNICODE_ENVIRONMENT
        | EXTENDED_STARTUPINFO_PRESENT;
    // SAFETY: a plain C struct, filled by the call.
    let mut pi: PROCESS_INFORMATION = unsafe { zeroed() };
    let ok = {
        // SAFETY: every pointer is valid and NUL-terminated where it must be, `line` is
        // writable, the attribute list and the handles it names outlive the call.
        unsafe {
            match &cmd.token {
                Some(token) => CreateProcessAsUserW(
                    raw(token),
                    app.as_ptr(),
                    line.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    1,
                    flags,
                    env.as_ptr().cast(),
                    cwd.as_ptr(),
                    &si.StartupInfo,
                    &mut pi,
                ),
                None => CreateProcessW(
                    app.as_ptr(),
                    line.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    1,
                    flags,
                    env.as_ptr().cast(),
                    cwd.as_ptr(),
                    &si.StartupInfo,
                    &mut pi,
                ),
            }
        }
    };
    drop(in_broker);
    if let Err(e) = check(ok) {
        if e.raw_os_error() == Some(ERROR_NOT_ENOUGH_QUOTA as i32) {
            return Err(crate::Refusal::io(PROCESS_LIMIT, e.to_string()));
        }
        return Err(e);
    }
    Ok(SuspendedProcess {
        process: Some(owned(pi.hProcess)?),
        thread: owned(pi.hThread)?,
        pid: pi.dwProcessId,
    })
}

pub(super) fn spawn(cmd: PlatformCommand, ctx: &SpawnCtx<'_>) -> io::Result<Spawned> {
    let (stdin_r, stdin_w) = io::pipe()?;
    let (stdout_r, stdout_w) = io::pipe()?;
    let (stderr_r, stderr_w) = io::pipe()?;
    let stdio = ChildStdio {
        stdin: &stdin_r,
        stdout: &stdout_w,
        stderr: &stderr_w,
    };
    let host = &super::job::group_job(ctx.group).host;
    let sp = host.broker.with(&host.job, |broker| {
        spawn_suspended(&cmd, stdio, priority(ctx.qos), broker)
    })?;
    // Our copies of the child's ends close here, so EOF reaches both sides.
    drop((stdin_r, stdout_w, stderr_w));
    let mut place = sp.placement();
    // An error drops `sp`, which terminates the suspended process.
    ctx.domain.place(ctx.group, ctx.leaf, &mut place)?;
    ctx.domain.placed(ctx.group, ctx.leaf, sp.pid)?;
    #[cfg(feature = "test-hooks")]
    if let Some(d) = ctx.resume_delay {
        std::thread::sleep(d);
    }
    let pid = sp.pid;
    let process = sp.resume()?;
    Ok(Spawned {
        pid,
        stdin: stdin_w,
        stdout: stdout_r,
        stderr: stderr_r,
        os: OsProc { process },
    })
}
