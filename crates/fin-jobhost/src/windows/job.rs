//! The Job Object kill domain (Decisions 14, 17, 20, 22).
//!
//! - **Host Job**, made at `Host::start`: `KILL_ON_JOB_CLOSE | DIE_ON_UNHANDLED_EXCEPTION`, no
//!   `BREAKAWAY_OK`, its one handle non-inheritable and held only here. When the host process
//!   dies, that handle closes and every process in the tree dies with it.
//! - **Group Job** per group, nested in the host Job by its first process: the same flags,
//!   plus `JOB_OBJECT_LIMIT_JOB_MEMORY` when the group has a limit; associated with the
//!   completion port under the group's key. Kill is `TerminateJobObject`, which ends its
//!   leaves too.
//! - **Leaf Job** per process, nested in its group Job: the process's `JobLimits`
//!   (`JOB_OBJECT_LIMIT_ACTIVE_PROCESS`, `JOB_OBJECT_UILIMIT_*`). The process's `Proc` holds it
//!   until the process exits.
//!
//! A process is born in the host Job (through the broker, `broker.rs`) or assigned to it, then
//! assigned to its group and leaf Jobs while suspended, so the nesting holds even when the host
//! itself runs in a terminal's or Electron's Job. At each exit the facts come from the group
//! Job: the port's memory message, a recorded violation, or a peak above the limit (the peak
//! includes the commit that was refused) means `oom` and ends the rest of the group, as a
//! cgroup's `memory.oom.group` would. Foreign processes are counted from each **leaf**'s
//! `TotalProcesses` less its own process (a group Job counts a process again when it moves on
//! into a nested leaf), less the console hosts Windows starts for console programs
//! (`conhost.rs`).

use std::collections::HashSet;
use std::io;
use std::mem::size_of;
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOBOBJECT_LIMIT_VIOLATION_INFORMATION, JOBOBJECTINFOCLASS, JobObjectBasicAccountingInformation,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation,
    JobObjectLimitViolationInformation, SetInformationJobObject,
};

use super::broker::Broker;
use super::iocp::Port;
use super::jobapi::{assign, create, in_job, query, set_limits, terminate};
use super::{check, raw};
use crate::domain::{ExitFacts, GroupHandle, KillDomain, LeafHandle, Placement};
use crate::spawner::JobLimits;
use crate::{GroupId, GroupSpec, HostOptions, KillTree};

/// `JobObjectMemoryUsageInformation` (Windows 10+): the job's current and peak committed memory.
const JOB_OBJECT_MEMORY_USAGE_INFORMATION: JOBOBJECTINFOCLASS = 28;

#[repr(C)]
#[derive(Default)]
struct MemoryUsage {
    job_memory: u64,
    peak_job_memory_used: u64,
}

/// The host Job and its spawn broker. Its handle is never inherited or duplicated; the domain
/// and its groups share it, and it closes when the host goes (or its process dies).
pub(crate) struct HostJob {
    pub job: OwnedHandle,
    pub broker: Broker,
}

/// A group's nested Job and its completion key.
pub(crate) struct GroupJob {
    pub job: OwnedHandle,
    pub key: usize,
    pub host: Arc<HostJob>,
    /// The port saw the group pass its memory limit, or an exit confirmed it.
    pub oom: AtomicBool,
    /// The leaves of its live processes, for the foreign count.
    leaves: Mutex<Vec<Weak<LeafJob>>>,
    /// Foreign processes of leaves already closed.
    closed_foreign: AtomicU32,
    /// Console hosts seen in the group (not foreign).
    conhosts: Mutex<HashSet<u32>>,
    pub memory_limit: Option<u64>,
}

impl GroupJob {
    /// A process appeared in the group (the port's hint, or a snapshot).
    pub fn noted(&self, pid: u32) {
        let mut seen = self.conhosts.lock().unwrap_or_else(|e| e.into_inner());
        if !seen.contains(&pid) && super::conhost::is_console_host(pid) {
            seen.insert(pid);
        }
    }

    /// Notes every live process of the group.
    pub fn scan(&self) {
        for pid in super::conhost::live_pids(&self.job) {
            self.noted(pid);
        }
    }

    fn conhosts(&self) -> u32 {
        self.conhosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len() as u32
    }

    /// The group passed its memory limit: record it and end every process in it.
    pub fn over_memory(&self) {
        self.oom.store(true, Ordering::Release);
        let _ = terminate(&self.job);
    }
}

/// A process's leaf Job.
pub(crate) struct LeafJob {
    pub job: OwnedHandle,
}

pub(crate) struct JobObjects {
    host: Arc<HostJob>,
    port: Arc<Port>,
    #[cfg(feature = "test-hooks")]
    hook: Option<crate::SuspendedHook>,
}

/// A leaf's processes beyond its own.
fn leaf_foreign(l: &LeafJob) -> u32 {
    query::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>(&l.job, JobObjectBasicAccountingInformation)
        .map_or(0, |a| a.TotalProcesses.saturating_sub(1))
}

pub(crate) fn group_job(group: &GroupHandle) -> &Arc<GroupJob> {
    group
        .os
        .downcast_ref::<Arc<GroupJob>>()
        .expect("a Job domain group")
}

fn leaf_job(leaf: &LeafHandle) -> &Arc<LeafJob> {
    leaf.os
        .downcast_ref::<Arc<LeafJob>>()
        .expect("a Job domain leaf")
}

impl JobObjects {
    pub fn new(opts: &HostOptions) -> io::Result<JobObjects> {
        let job = create()?;
        set_limits(
            &job,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
            None,
            None,
        )?;
        let port = Port::new(opts.log.clone())?;
        #[cfg(feature = "test-hooks")]
        let start = !opts.port_stopped_for_test;
        #[cfg(not(feature = "test-hooks"))]
        let start = true;
        if start {
            port.start()?;
        }
        Ok(JobObjects {
            host: Arc::new(HostJob {
                job,
                broker: Broker::new(opts.log.clone()),
            }),
            port,
            #[cfg(feature = "test-hooks")]
            hook: opts.suspended_hook_for_test.clone(),
        })
    }

    #[cfg(feature = "test-hooks")]
    fn run_hook(&self, g: &GroupJob, l: &LeafJob, p: &Placement) -> io::Result<()> {
        use windows_sys::Win32::System::Threading::{ResumeThread, SuspendThread};
        let Some(hook) = &self.hook else {
            return Ok(());
        };
        let process = p.process as HANDLE;
        let thread = p.thread as HANDLE;
        // SAFETY: the suspended process's primary thread; one suspend, one resume.
        let suspend_count = unsafe {
            let prev = SuspendThread(thread);
            ResumeThread(thread);
            prev
        };
        hook(&crate::SuspendedProbe {
            pid: p.pid,
            in_host: in_job(process, &self.host.job),
            in_group: in_job(process, &g.job),
            in_leaf: in_job(process, &l.job),
            suspend_count,
        })
    }
}

impl Drop for JobObjects {
    fn drop(&mut self) {
        // The port thread first; the host Job's handle closes after, killing what is left.
        self.port.stop();
    }
}

impl KillDomain for JobObjects {
    fn kill_tree(&self) -> KillTree {
        KillTree::JobObject
    }

    fn leaf_limits(&self) -> bool {
        true
    }

    fn kernel_memory(&self) -> bool {
        true
    }

    fn open_group(&self, id: GroupId, spec: &GroupSpec) -> io::Result<GroupHandle> {
        let job = create()?;
        set_limits(
            &job,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
            None,
            spec.memory_bytes,
        )?;
        let key = usize::try_from(id.0).map_err(|_| io::Error::other("group id overflow"))?;
        let g = Arc::new(GroupJob {
            job,
            key,
            host: self.host.clone(),
            oom: AtomicBool::new(false),
            leaves: Mutex::default(),
            closed_foreign: AtomicU32::new(0),
            conhosts: Mutex::default(),
            memory_limit: spec.memory_bytes,
        });
        self.port.register(&g)?;
        Ok(GroupHandle::new(id, spec, Box::new(g)))
    }

    fn open_leaf(&self, group: &GroupHandle, limits: &JobLimits) -> io::Result<LeafHandle> {
        let job = create()?;
        set_limits(
            &job,
            JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
            limits.active_processes,
            None,
        )?;
        if let Some(ui) = limits.ui {
            let info = JOBOBJECT_BASIC_UI_RESTRICTIONS {
                UIRestrictionsClass: ui.0,
            };
            // SAFETY: `info` is the struct this class takes, with its size.
            check(unsafe {
                SetInformationJobObject(
                    raw(&job),
                    JobObjectBasicUIRestrictions,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
                )
            })?;
        }
        let leaf = Arc::new(LeafJob { job });
        let mut leaves = group_job(group)
            .leaves
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        leaves.retain(|l| l.strong_count() > 0);
        leaves.push(Arc::downgrade(&leaf));
        Ok(LeafHandle { os: Box::new(leaf) })
    }

    /// Assigns the suspended process to the host, group and leaf Jobs, in that order.
    fn place(&self, group: &GroupHandle, leaf: &LeafHandle, p: &mut Placement) -> io::Result<()> {
        let (g, l) = (group_job(group), leaf_job(leaf));
        let process = p.process as HANDLE;
        assign(&self.host.job, process)?;
        assign(&g.job, process)?;
        assign(&l.job, process)?;
        #[cfg(feature = "test-hooks")]
        self.run_hook(g, l, p)?;
        Ok(())
    }

    /// A console host starts during its program's initialisation, so by `hello` it is there.
    fn hello(&self, group: &GroupHandle, _pid: u32) {
        group_job(group).scan();
    }

    fn kill(&self, group: &GroupHandle) -> io::Result<()> {
        terminate(&group_job(group).job)
    }

    fn memory(&self, group: &GroupHandle) -> Option<u64> {
        query::<MemoryUsage>(&group_job(group).job, JOB_OBJECT_MEMORY_USAGE_INFORMATION)
            .ok()
            .map(|m| m.job_memory)
    }

    /// Counted per leaf (see the module notes), so `spawned` is not needed.
    fn foreign(&self, group: &GroupHandle, _spawned: u32) -> Option<u32> {
        let g = group_job(group);
        g.scan();
        let live: u32 = g
            .leaves
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(Weak::upgrade)
            .map(|l| leaf_foreign(&l))
            .sum();
        Some((g.closed_foreign.load(Ordering::Acquire) + live).saturating_sub(g.conhosts()))
    }

    fn exit_facts(&self, group: &GroupHandle) -> ExitFacts {
        let g = group_job(group);
        let violated = query::<JOBOBJECT_LIMIT_VIOLATION_INFORMATION>(
            &g.job,
            JobObjectLimitViolationInformation,
        )
        .is_ok_and(|v| v.ViolationLimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0);
        let acct = query::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>(
            &g.job,
            JobObjectBasicAccountingInformation,
        );
        let ext = query::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(
            &g.job,
            JobObjectExtendedLimitInformation,
        );
        // A refused commit still raises the peak, so a peak above the limit is a violation.
        let over = match (&ext, g.memory_limit) {
            (Ok(e), Some(limit)) => e.PeakJobMemoryUsed as u64 > limit,
            _ => false,
        };
        let oom = violated || over || g.oom.load(Ordering::Acquire);
        if oom {
            g.over_memory();
        }
        ExitFacts {
            oom,
            total_processes: acct.ok().map(|a| a.TotalProcesses),
            memory_peak: ext.ok().map(|e| e.PeakJobMemoryUsed as u64),
        }
    }

    /// The group is released: its leaf Job's handle closes here, on the spawn thread (the
    /// leaf has no kill-on-close; its group's kill covers anything left in it).
    fn close_leaf(&self, group: &GroupHandle, leaf: LeafHandle) {
        let g = group_job(group);
        let l = leaf_job(&leaf);
        let mut leaves = g.leaves.lock().unwrap_or_else(|e| e.into_inner());
        g.closed_foreign
            .fetch_add(leaf_foreign(l), Ordering::AcqRel);
        leaves.retain(|w| w.strong_count() > 0 && !std::ptr::eq(w.as_ptr(), Arc::as_ptr(l)));
        drop(leaves);
        drop(leaf);
    }

    /// The group is closed and empty: any process a member left behind ends now (other
    /// holders of the handle may keep the Job open a little longer; its kill-on-close covers
    /// the host's death).
    fn close_group(&self, group: &GroupHandle) {
        let g = group_job(group);
        self.port.unregister(g.key);
        let _ = terminate(&g.job);
    }
}
