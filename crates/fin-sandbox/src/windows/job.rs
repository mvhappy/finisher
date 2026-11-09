//! The container's job object (plan 7C Decision 16, S9 Windows point 4): kill-on-close, with
//! the policy's process and memory limits, `DIE_ON_UNHANDLED_EXCEPTION`, UI restrictions, and
//! **never** `BREAKAWAY_OK` or `SILENT_BREAKAWAY_OK` (S9: `CREATE_BREAKAWAY_FROM_JOB` is refused
//! only while neither is set). Assigned at creation through `PROC_THREAD_ATTRIBUTE_JOB_LIST`,
//! so no code of the process runs outside it. A completion port reports the limits it hits.

use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::io::OwnedHandle;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectAssociateCompletionPortInformation, JobObjectBasicAndIoAccountingInformation,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
};

use super::{owned, raw};
use crate::policy::Limits;

// `JOB_OBJECT_MSG_*` (winnt.h; windows-sys puts them behind `Win32_System_SystemServices`).
const JOB_OBJECT_MSG_ACTIVE_PROCESS_LIMIT: u32 = 3;
const JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT: u32 = 9;
const JOB_OBJECT_MSG_JOB_MEMORY_LIMIT: u32 = 10;

/// The UI restrictions of every container job: no USER handles of other jobs, no clipboard,
/// no desktop switch or creation, no system settings, no log-off. Global atoms stay (some
/// toolkits need them).
pub const UI_LIMITS: u32 = JOB_OBJECT_UILIMIT_DESKTOP
    | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
    | JOB_OBJECT_UILIMIT_EXITWINDOWS
    | JOB_OBJECT_UILIMIT_HANDLES
    | JOB_OBJECT_UILIMIT_READCLIPBOARD
    | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
    | JOB_OBJECT_UILIMIT_WRITECLIPBOARD;

/// The flags that would let a process leave the job: never set.
pub const BREAKAWAY: u32 = JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;

/// A limit the job reported (a `sandbox.violation`, plan 7C Decision 24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// A process creation refused by the active-process limit (`ERROR_NOT_ENOUGH_QUOTA`).
    ProcessLimit,
    /// The job's (or a process's) memory limit.
    MemoryLimit,
}

impl Violation {
    /// The `op` of the `sandbox.violation` entry.
    pub fn op(self) -> &'static str {
        match self {
            Violation::ProcessLimit => "processLimit",
            Violation::MemoryLimit => "memoryLimit",
        }
    }
}

/// A job's limits as Windows reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobLimits {
    pub flags: u32,
    pub active_processes: Option<u32>,
    pub job_memory: Option<u64>,
}

/// The job object of one container launch.
pub struct ContainerJob {
    job: OwnedHandle,
    port: OwnedHandle,
}

/// The limit flags and values for `limits` (pure, for tests).
pub fn limit_flags(limits: &Limits) -> (u32, u32, usize) {
    let mut flags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    let mut procs = 0;
    let mut memory = 0;
    if let Some(n) = limits.processes {
        flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        procs = n;
    }
    if let Some(m) = limits.memory {
        flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
        memory = usize::try_from(m).unwrap_or(usize::MAX);
    }
    debug_assert_eq!(flags & BREAKAWAY, 0);
    (flags, procs, memory)
}

fn set<T>(job: HANDLE, class: i32, value: &T) -> io::Result<()> {
    // SAFETY: a job handle with JOB_OBJECT_SET_ATTRIBUTES and a value of the class's type.
    let ok = unsafe {
        SetInformationJobObject(
            job,
            class,
            (value as *const T).cast(),
            size_of::<T>() as u32,
        )
    };
    super::check(ok)
}

impl ContainerJob {
    pub fn new(limits: &Limits) -> io::Result<ContainerJob> {
        // SAFETY: an unnamed job with the default (owner-only) security.
        let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let (flags, procs, memory) = limit_flags(limits);
        // SAFETY: a plain C struct; zeros are its defaults.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        info.BasicLimitInformation.LimitFlags = flags;
        info.BasicLimitInformation.ActiveProcessLimit = procs;
        info.JobMemoryLimit = memory;
        set(raw(&job), JobObjectExtendedLimitInformation, &info)?;
        let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: UI_LIMITS,
        };
        set(raw(&job), JobObjectBasicUIRestrictions, &ui)?;
        // SAFETY: a new completion port, no file.
        let port =
            owned(unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, null_mut(), 0, 1) })?;
        let assoc = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
            CompletionKey: null_mut(),
            CompletionPort: raw(&port),
        };
        set(
            raw(&job),
            JobObjectAssociateCompletionPortInformation,
            &assoc,
        )?;
        Ok(ContainerJob { job, port })
    }

    pub fn handle(&self) -> HANDLE {
        raw(&self.job)
    }

    /// The limits reported since the last call (does not wait).
    pub fn violations(&self) -> Vec<Violation> {
        let mut out = Vec::new();
        loop {
            let (mut msg, mut key, mut ov) = (0u32, 0usize, null_mut::<OVERLAPPED>());
            // SAFETY: our port and valid out-pointers; a zero timeout.
            let ok = unsafe {
                GetQueuedCompletionStatus(raw(&self.port), &mut msg, &mut key, &mut ov, 0)
            };
            if ok == 0 {
                return out;
            }
            match msg {
                JOB_OBJECT_MSG_ACTIVE_PROCESS_LIMIT => out.push(Violation::ProcessLimit),
                JOB_OBJECT_MSG_JOB_MEMORY_LIMIT | JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT => {
                    out.push(Violation::MemoryLimit)
                }
                _ => {}
            }
        }
    }

    /// Read and write operations of every process the job has held (Node's piped-stdio
    /// backstop: no change means no progress).
    pub fn io_operations(&self) -> u64 {
        // SAFETY: a plain C struct; zeros are its defaults.
        let mut info: JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        // SAFETY: our job handle (JOB_OBJECT_QUERY) and a value of the class's type.
        let ok = unsafe {
            QueryInformationJobObject(
                raw(&self.job),
                JobObjectBasicAndIoAccountingInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        };
        if ok == 0 {
            return 0;
        }
        // Reads and writes only: a Node spawn stuck on piped stdio retries `CreateNamedPipe`
        // forever, and those calls count as "other" operations (measured on the S9 host).
        let io = info.IoInfo;
        io.ReadOperationCount + io.WriteOperationCount
    }

    /// The limits the job has, read back from Windows.
    pub fn limits(&self) -> io::Result<JobLimits> {
        // SAFETY: a plain C struct; zeros are its defaults.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        // SAFETY: our job handle and a value of the class's type.
        let ok = unsafe {
            QueryInformationJobObject(
                raw(&self.job),
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                null_mut(),
            )
        };
        super::check(ok)?;
        let flags = info.BasicLimitInformation.LimitFlags;
        Ok(JobLimits {
            flags,
            active_processes: (flags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0)
                .then_some(info.BasicLimitInformation.ActiveProcessLimit),
            job_memory: (flags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0)
                .then_some(info.JobMemoryLimit as u64),
        })
    }

    /// Ends every process of the job.
    pub fn terminate(&self, code: u32) {
        // SAFETY: our job handle with JOB_OBJECT_TERMINATE.
        unsafe { TerminateJobObject(raw(&self.job), code) };
    }
}
