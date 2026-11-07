//! The spawn broker: how a child gets its three pipe ends without the host ever holding an
//! inheritable handle (Decision 10, as built).
//!
//! Making the child's ends inheritable in the host only around `CreateProcessW` still leaves a
//! window: any other `CreateProcess` with `bInheritHandles` in the host process (libuv's, or
//! `std`'s) during that call inherits them, and the process tests show it does under load. So
//! each host keeps one **broker**: a copy of the host's own executable, created suspended and
//! never resumed (no code of it ever runs), in the host Job so it dies with the host. For each
//! spawn the three ends are duplicated *into the broker* as inheritable handles, the child is
//! created with `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS` = the broker (so it inherits from the
//! broker's handle table, restricted further by `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`), and the
//! broker's copies are closed right after. The host's own handles stay non-inheritable
//! throughout. The child is born in the broker's Jobs (the host Job, and any Job the host runs
//! in); it is then assigned to its group and leaf Jobs as before.
//!
//! If no broker can be made, the spawn falls back to the flip around `CreateProcessW`
//! (`inherit.rs`), and the host logs it once.

use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{
    DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DETACHED_PROCESS, GetCurrentProcess, PROCESS_INFORMATION,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};

use super::env_block::{command_line, wide};
use super::{check, owned, raw};
use crate::LogFn;

pub(crate) struct Broker {
    process: Mutex<Option<OwnedHandle>>,
    warned: AtomicBool,
    log: LogFn,
}

/// A suspended copy of this executable in `host_job`.
fn make(host_job: &OwnedHandle) -> io::Result<OwnedHandle> {
    let exe = std::env::current_exe()?;
    let app = wide(exe.as_os_str())?;
    let mut line = command_line(&exe, &[])?;
    // SAFETY: a plain C struct; zeros are its defaults.
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = size_of::<STARTUPINFOW>() as u32;
    // SAFETY: a plain C struct, filled by the call.
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: valid, NUL-terminated strings; no handle is inherited.
    check(unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_SUSPENDED | DETACHED_PROCESS,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        )
    })?;
    let process = owned(pi.hProcess)?;
    drop(owned(pi.hThread)?);
    // SAFETY: our Job and the process just made.
    if let Err(e) = check(unsafe { AssignProcessToJobObject(raw(host_job), raw(&process)) }) {
        // SAFETY: our own suspended process.
        unsafe { TerminateProcess(raw(&process), 1) };
        return Err(e);
    }
    Ok(process)
}

fn alive(p: &OwnedHandle) -> bool {
    // SAFETY: a process handle we own.
    unsafe { WaitForSingleObject(raw(p), 0) == WAIT_TIMEOUT }
}

impl Broker {
    pub fn new(log: LogFn) -> Broker {
        Broker {
            process: Mutex::new(None),
            warned: AtomicBool::new(false),
            log,
        }
    }

    /// Runs `f` with the broker (made, or remade if it died, in `host_job`), or with `None`
    /// when none can be made.
    pub fn with<R>(&self, host_job: &OwnedHandle, f: impl FnOnce(Option<HANDLE>) -> R) -> R {
        let mut slot = self.process.lock().unwrap_or_else(|e| e.into_inner());
        if !slot.as_ref().is_some_and(alive) {
            *slot = match make(host_job) {
                Ok(p) => Some(p),
                Err(e) => {
                    if !self.warned.swap(true, Ordering::Relaxed) {
                        (self.log)(&format!(
                            "no spawn broker ({e}); pipe ends are made inheritable around each spawn"
                        ));
                    }
                    None
                }
            };
        }
        f(slot.as_ref().map(raw))
    }
}

/// Copies of the child's ends in the broker, inheritable there; closed there when dropped.
pub(crate) struct InBroker {
    broker: HANDLE,
    pub handles: Vec<HANDLE>,
}

impl InBroker {
    pub fn new(broker: HANDLE, ours: &[HANDLE]) -> io::Result<InBroker> {
        let mut me = InBroker {
            broker,
            handles: Vec::with_capacity(ours.len()),
        };
        for &h in ours {
            let mut theirs: HANDLE = std::ptr::null_mut();
            // SAFETY: our handle, copied into the broker (which we hold with full access).
            check(unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    h,
                    broker,
                    &mut theirs,
                    0,
                    1,
                    DUPLICATE_SAME_ACCESS,
                )
            })?;
            me.handles.push(theirs);
        }
        Ok(me)
    }
}

impl Drop for InBroker {
    fn drop(&mut self) {
        for &h in &self.handles {
            // SAFETY: closes a handle in the broker's table that we put there.
            unsafe {
                DuplicateHandle(
                    self.broker,
                    h,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    DUPLICATE_CLOSE_SOURCE,
                )
            };
        }
    }
}
