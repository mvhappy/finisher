//! The Win32 Job Object calls the Job domain makes.

use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::io::OwnedHandle;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECTINFOCLASS, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};

use super::{KILLED_EXIT_CODE, check, owned, raw};

pub(super) fn create() -> io::Result<OwnedHandle> {
    // SAFETY: no attributes (so the handle is not inheritable) and no name.
    owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })
}

/// Sets a Job's limit flags, active-process limit and job memory limit.
pub(super) fn set_limits(
    job: &OwnedHandle,
    mut flags: JOB_OBJECT_LIMIT,
    active: Option<u32>,
    memory: Option<u64>,
) -> io::Result<()> {
    // SAFETY: a plain C struct; all zeros is "no limits".
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    if let Some(n) = active {
        flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        info.BasicLimitInformation.ActiveProcessLimit = n;
    }
    if let Some(bytes) = memory {
        flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
        info.JobMemoryLimit = usize::try_from(bytes).unwrap_or(usize::MAX);
    }
    info.BasicLimitInformation.LimitFlags = flags;
    // SAFETY: `info` is the struct this class takes, with its size.
    check(unsafe {
        SetInformationJobObject(
            raw(job),
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })
}

/// One `QueryInformationJobObject` class into its struct.
pub(super) fn query<T>(job: &OwnedHandle, class: JOBOBJECTINFOCLASS) -> io::Result<T> {
    // SAFETY: every class queried here fills a plain C struct, for which zeros are valid.
    let mut out: T = unsafe { zeroed() };
    // SAFETY: `out` is the struct this class fills, with its size.
    check(unsafe {
        QueryInformationJobObject(
            raw(job),
            class,
            (&raw mut out).cast(),
            size_of::<T>() as u32,
            std::ptr::null_mut(),
        )
    })?;
    Ok(out)
}

/// One class of the host's own innermost Job (a null Job handle); an error when the host
/// runs in no Job.
pub(super) fn query_own<T>(class: JOBOBJECTINFOCLASS) -> io::Result<T> {
    // SAFETY: as in `query`; a null handle names the calling process's innermost Job.
    let mut out: T = unsafe { zeroed() };
    // SAFETY: `out` is the struct this class fills, with its size.
    check(unsafe {
        QueryInformationJobObject(
            std::ptr::null_mut(),
            class,
            (&raw mut out).cast(),
            size_of::<T>() as u32,
            std::ptr::null_mut(),
        )
    })?;
    Ok(out)
}

pub(super) fn terminate(job: &OwnedHandle) -> io::Result<()> {
    // SAFETY: a Job handle we own, with JOB_OBJECT_TERMINATE access.
    check(unsafe { TerminateJobObject(raw(job), KILLED_EXIT_CODE) })
}

/// Assigns the process to `job`, unless it is already in it (born there).
pub(super) fn assign(job: &OwnedHandle, process: HANDLE) -> io::Result<()> {
    if in_job(process, job) {
        return Ok(());
    }
    // SAFETY: a Job handle we own and the suspended process's handle.
    check(unsafe { AssignProcessToJobObject(raw(job), process) })
}

pub(super) fn in_job(process: HANDLE, job: &OwnedHandle) -> bool {
    let mut r = 0;
    // SAFETY: both handles are valid; `r` is a valid out-pointer.
    unsafe { IsProcessInJob(process, raw(job), &mut r) != 0 && r != 0 }
}
