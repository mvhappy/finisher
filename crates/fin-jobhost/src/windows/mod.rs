//! Windows (Decisions 10, 14, 17, 18, 20–23): a kill-on-close **host Job**, one nested
//! **group Job** per group and one **leaf Job** per process ([`job`]); every process is created
//! suspended in the host Job, assigned to its group and leaf Jobs and only then resumed
//! ([`spawn`]); the three child pipe ends reach it through a never-running broker process, so
//! the host never holds an inheritable handle ([`broker`]; [`inherit`] is the fallback); an
//! advisory completion port ([`iocp`]) ends a group at once when it passes its memory limit.
//! Exits come from the process handle the waiter waits on; the Jobs confirm the facts.
//!
//! There are no signals: a process the host kills exits with [`KILLED_EXIT_CODE`], and
//! `ExitInfo.signal` is always `None`.

use std::io::{self, PipeReader, PipeWriter};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};

use crate::domain::{GroupHandle, KillDomain, LeafHandle};
use crate::exit::RawExit;
use crate::spawner::{PlatformCommand, Qos};
use crate::{HostError, HostOptions};

mod broker;
mod conhost;
mod env_block;
mod inherit;
mod iocp;
pub(crate) mod job;
mod jobapi;
mod spawn;

/// The exit code of a process the host killed (`TerminateJobObject`, `TerminateProcess`):
/// 137, as a shell reports a `SIGKILL`ed process. Windows has no signals.
pub const KILLED_EXIT_CODE: u32 = 137;

/// The OS's part of a process: the handle its waiter waits on.
pub(crate) struct OsProc {
    pub process: OwnedHandle,
}

/// A started process and the host's ends of its pipes.
pub(crate) struct Spawned {
    pub pid: u32,
    pub stdin: PipeWriter,
    pub stdout: PipeReader,
    pub stderr: PipeReader,
    pub os: OsProc,
}

/// What a spawn needs besides the command.
pub(crate) struct SpawnCtx<'a> {
    pub domain: &'a dyn KillDomain,
    pub group: &'a GroupHandle,
    pub leaf: &'a LeafHandle,
    pub qos: Qos,
    #[allow(dead_code)] // Linux's getppid check; Windows has none.
    pub host_pid: u32,
    /// How long the process stays suspended after its placement (tests).
    #[cfg(feature = "test-hooks")]
    pub resume_delay: Option<std::time::Duration>,
}

/// The Job Object domain: the host Job and its completion port are made here, at
/// `Host::start`.
pub(crate) fn default_domain(opts: &HostOptions) -> Result<Box<dyn KillDomain>, HostError> {
    job::JobObjects::new(opts)
        .map(|d| Box::new(d) as Box<dyn KillDomain>)
        .map_err(HostError::Spawn)
}

/// Starts `cmd` suspended, places it in its Jobs, then resumes it.
pub(crate) fn spawn(cmd: PlatformCommand, ctx: &SpawnCtx<'_>) -> io::Result<Spawned> {
    spawn::spawn(cmd, ctx)
}

/// Waits on the process handle. `reap` means nothing here (no zombies).
pub(crate) fn wait(os: &OsProc, _pid: u32, _reap: bool) -> io::Result<RawExit> {
    let h = os.process.as_raw_handle() as HANDLE;
    // SAFETY: a process handle we own, with SYNCHRONIZE access.
    if unsafe { WaitForSingleObject(h, INFINITE) } != WAIT_OBJECT_0 {
        return Err(io::Error::last_os_error());
    }
    let mut code = 0u32;
    // SAFETY: as above, with PROCESS_QUERY_LIMITED_INFORMATION; `code` is a valid out-pointer.
    check(unsafe { GetExitCodeProcess(h, &mut code) })?;
    Ok(RawExit {
        code: Some(i64::from(code)),
        signal: None,
    })
}

/// Nothing to reap on Windows.
pub(crate) fn reap(_pid: u32) {}

/// `Ok` for a non-zero `BOOL`, else the last OS error.
pub(crate) fn check(ok: windows_sys::core::BOOL) -> io::Result<()> {
    if ok != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Takes ownership of a handle a Win32 call returned (null or `INVALID_HANDLE_VALUE`: the
/// last OS error).
pub(crate) fn owned(h: HANDLE) -> io::Result<OwnedHandle> {
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle nobody else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(h as _) })
}

/// The raw value of a handle we keep.
pub(crate) fn raw(h: &impl AsRawHandle) -> HANDLE {
    h.as_raw_handle() as HANDLE
}
