//! Kill domains (Decision 17): the per-OS trait the host drives, on its spawn thread.
//!
//! A **group** is one kill domain: a process group (Unix, this task), a delegated cgroup
//! (Linux, Task 4) or a nested Job (Windows, `windows/job.rs`). Every process also has a **leaf** in its
//! group, which carries its `JobLimits` (a leaf Job or leaf cgroup; nothing with process
//! groups). Each implementation keeps its own state in the handles' `os` box and downcasts
//! it, so Tasks 4 and 5 add theirs without touching this file.
//!
//! The host calls, per group: `open_group` at its first spawn; per process `open_leaf`,
//! `place` (before the process runs its first instruction), `placed` (once it exists); `kill`
//! for `kill_group` and the timers; `hello` at the process's `hello`; `memory` at `ready`;
//! `foreign` and `exit_facts` at each exit; once the group is closed and empty `close_leaf` for
//! each process, then `close_group`, **before** the Unix leader is reaped (so a watchdog's `-g`
//! precedes the reap, Decision 15).

use std::any::Any;
use std::io;
#[cfg(unix)]
use std::sync::atomic::AtomicI32;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::spawner::JobLimits;
use crate::{GroupId, GroupSpec};

/// An open group. `os` is the implementation's own state.
#[allow(dead_code)] // `spec` is read by the cgroup domain (Task 4), `os` by the Job domain.
pub(crate) struct GroupHandle {
    pub id: GroupId,
    pub spec: GroupSpec,
    /// Unix: the group's process-group id, 0 until its first process exists (the first
    /// process creates it; later ones join it). Set by the host.
    #[cfg(unix)]
    pub pgid: AtomicI32,
    /// The highest figure the host's sampler saw (Decision 20), 0 before the first sample.
    pub sampled_peak: AtomicU64,
    pub os: Box<dyn Any + Send + Sync>,
}

impl GroupHandle {
    pub fn new(id: GroupId, spec: &GroupSpec, os: Box<dyn Any + Send + Sync>) -> GroupHandle {
        GroupHandle {
            id,
            spec: spec.clone(),
            #[cfg(unix)]
            pgid: AtomicI32::new(0),
            sampled_peak: AtomicU64::new(0),
            os,
        }
    }

    #[cfg(unix)]
    pub fn pgid(&self) -> i32 {
        self.pgid.load(Ordering::Acquire)
    }

    pub fn note_sample(&self, bytes: u64) {
        self.sampled_peak.fetch_max(bytes, Ordering::AcqRel);
    }

    pub fn sampled_peak(&self) -> Option<u64> {
        Some(self.sampled_peak.load(Ordering::Acquire)).filter(|&b| b > 0)
    }
}

/// A process's leaf in its group.
#[cfg_attr(unix, allow(dead_code))] // read by the cgroup domain (Task 4).
pub(crate) struct LeafHandle {
    pub os: Box<dyn Any + Send + Sync>,
}

impl LeafHandle {
    #[cfg_attr(windows, allow(dead_code))]
    pub fn none() -> LeafHandle {
        LeafHandle { os: Box::new(()) }
    }
}

/// What `place` may arrange.
///
/// - Linux: called **before the fork**. `pgid` is the group's (0: this process creates it);
///   `child_writes` are written in the child, in order, after `setpgid` and
///   `PR_SET_PDEATHSIG` and before `exec` (a leaf cgroup's `cgroup.procs` ← `"0"`, through an
///   fd opened here: nothing is allocated in the child).
/// - macOS: called on the **suspended** process (`pid` set), before it is resumed.
/// - Windows: on the suspended process (assigned to the host, group and leaf Jobs), before
///   `ResumeThread`.
#[cfg(target_os = "linux")]
pub(crate) struct Placement {
    pub pgid: i32,
    pub child_writes: Vec<(std::os::fd::OwnedFd, &'static [u8])>,
}

#[cfg(target_os = "macos")]
#[allow(dead_code)] // read by the watchdog registration (Task 4)
pub(crate) struct Placement {
    pub pgid: i32,
    pub pid: u32,
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub(crate) struct Placement {
    pub pgid: i32,
}

#[cfg(windows)]
pub(crate) struct Placement {
    /// The suspended process (`HANDLE`), owned by the spawn code.
    pub process: std::os::windows::io::RawHandle,
    /// Its primary thread, still suspended (`HANDLE`), owned by the spawn code.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub thread: std::os::windows::io::RawHandle,
    /// Its pid (the suspended-spawn probe of test-hooks builds reads it).
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub pid: u32,
}

/// Facts about a group confirmed at a process's exit (Decisions 14, 20, 22).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ExitFacts {
    /// The kernel enforced the group's memory limit (cgroup `oom_kill`, a Job violation).
    pub oom: bool,
    /// Every process the group ever held (Windows `TotalProcesses`).
    pub total_processes: Option<u32>,
    /// The group's peak memory, where the kernel keeps one.
    pub memory_peak: Option<u64>,
}

/// One OS's kill domains. Every method runs on the spawn thread except `hello`, `memory`,
/// `foreign` and `exit_facts`, which a waiter or a reader may call.
pub(crate) trait KillDomain: Send + Sync {
    /// The kill tree this domain is.
    fn kill_tree(&self) -> crate::KillTree;
    /// Whether `JobLimits` are enforced.
    fn leaf_limits(&self) -> bool;
    /// Whether a group's memory limit is enforced by the kernel.
    fn kernel_memory(&self) -> bool;
    /// The host watchdog this domain registers its groups with (Decision 15).
    #[cfg(unix)]
    fn watchdog(&self) -> Option<&crate::unix::watchdog::Watchdog> {
        None
    }

    fn open_group(&self, id: GroupId, spec: &GroupSpec) -> io::Result<GroupHandle>;
    fn open_leaf(&self, group: &GroupHandle, limits: &JobLimits) -> io::Result<LeafHandle>;
    /// Before the process runs (see [`Placement`]).
    fn place(&self, group: &GroupHandle, leaf: &LeafHandle, p: &mut Placement) -> io::Result<()>;
    /// The process exists with `pid` (Unix: in the group's pgid, already set on `group`;
    /// macOS: still suspended). The watchdog registration point. An error kills it.
    fn placed(&self, _group: &GroupHandle, _leaf: &LeafHandle, _pid: u32) -> io::Result<()> {
        Ok(())
    }
    /// The process sent `hello`: its own start-up is done (on its stdout reader). Windows
    /// recognises the process's console host here.
    fn hello(&self, _group: &GroupHandle, _pid: u32) {}
    /// Every process carrying the Seatbelt `mark` belongs to the group (before the process
    /// that carries it first is spawned); its kill and its close take them all.
    #[cfg(target_os = "macos")]
    fn mark(&self, _group: &GroupHandle, _mark: &str) {}
    /// Kills every process of the group.
    fn kill(&self, group: &GroupHandle) -> io::Result<()>;
    /// The group's current memory figure (Decision 20), if this domain measures one.
    fn memory(&self, group: &GroupHandle) -> Option<u64>;
    /// Processes in the group beyond the `spawned` ones the host started (Decision 22).
    fn foreign(&self, group: &GroupHandle, spawned: u32) -> Option<u32>;
    fn exit_facts(&self, group: &GroupHandle) -> ExitFacts;
    /// Whether a memory limit of a Job the host itself runs in refused a commit since the
    /// last time this said so (Windows; asked only for an abnormal exit the group's own facts
    /// do not explain).
    fn outer_oom(&self) -> bool {
        false
    }
    /// The spawn broker's pid and image (Windows tests).
    #[cfg(all(windows, feature = "test-hooks"))]
    fn broker_for_test(&self) -> Option<(u32, std::path::PathBuf)> {
        None
    }
    /// Releases a process's leaf: once its group is closed and empty (before `close_group`),
    /// or at once when the spawn failed. The default does nothing.
    fn close_leaf(&self, _group: &GroupHandle, _leaf: LeafHandle) {}
    /// The group is closed and empty: release it (always called, whoever else still holds a
    /// reference to the handle). On Unix the host reaps the leader after.
    fn close_group(&self, group: &GroupHandle);
}

/// A recording double over another domain: tests check the call order.
#[cfg(test)]
pub(crate) mod double {
    use super::*;
    use std::sync::{Arc, Mutex};

    pub(crate) struct Recording<D> {
        pub inner: D,
        pub calls: Arc<Mutex<Vec<String>>>,
        /// How many `placed` calls fail (from the first).
        pub fail_placed: std::sync::atomic::AtomicU32,
    }

    impl<D> Recording<D> {
        pub fn new(inner: D, calls: Arc<Mutex<Vec<String>>>) -> Recording<D> {
            Recording {
                inner,
                calls,
                fail_placed: Default::default(),
            }
        }
    }

    impl<D: KillDomain> Recording<D> {
        fn note(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl<D: KillDomain> KillDomain for Recording<D> {
        fn kill_tree(&self) -> crate::KillTree {
            self.inner.kill_tree()
        }
        fn leaf_limits(&self) -> bool {
            self.inner.leaf_limits()
        }
        fn kernel_memory(&self) -> bool {
            self.inner.kernel_memory()
        }
        #[cfg(unix)]
        fn watchdog(&self) -> Option<&crate::unix::watchdog::Watchdog> {
            self.inner.watchdog()
        }
        fn open_group(&self, id: GroupId, spec: &GroupSpec) -> io::Result<GroupHandle> {
            self.note(format!("open_group {id}"));
            self.inner.open_group(id, spec)
        }
        fn open_leaf(&self, group: &GroupHandle, limits: &JobLimits) -> io::Result<LeafHandle> {
            self.note(format!("open_leaf {}", group.id));
            self.inner.open_leaf(group, limits)
        }
        fn place(&self, g: &GroupHandle, l: &LeafHandle, p: &mut Placement) -> io::Result<()> {
            self.note(format!("place {}", g.id));
            self.inner.place(g, l, p)
        }
        fn placed(&self, g: &GroupHandle, l: &LeafHandle, pid: u32) -> io::Result<()> {
            self.note(format!("placed {}", g.id));
            use std::sync::atomic::Ordering::{AcqRel, Acquire};
            let n = self.fail_placed.load(Acquire);
            if n > 0
                && self
                    .fail_placed
                    .compare_exchange(n, n - 1, AcqRel, Acquire)
                    .is_ok()
            {
                return Err(io::Error::other("placed fails (test)"));
            }
            self.inner.placed(g, l, pid)
        }
        fn hello(&self, group: &GroupHandle, pid: u32) {
            self.inner.hello(group, pid)
        }
        #[cfg(target_os = "macos")]
        fn mark(&self, group: &GroupHandle, mark: &str) {
            self.inner.mark(group, mark)
        }
        fn kill(&self, group: &GroupHandle) -> io::Result<()> {
            self.note(format!("kill {}", group.id));
            self.inner.kill(group)
        }
        fn memory(&self, group: &GroupHandle) -> Option<u64> {
            self.inner.memory(group)
        }
        fn foreign(&self, group: &GroupHandle, spawned: u32) -> Option<u32> {
            self.inner.foreign(group, spawned)
        }
        fn exit_facts(&self, group: &GroupHandle) -> ExitFacts {
            self.note(format!("exit_facts {}", group.id));
            self.inner.exit_facts(group)
        }
        fn outer_oom(&self) -> bool {
            self.inner.outer_oom()
        }
        fn close_leaf(&self, group: &GroupHandle, leaf: LeafHandle) {
            self.note(format!("close_leaf {}", group.id));
            self.inner.close_leaf(group, leaf)
        }
        fn close_group(&self, group: &GroupHandle) {
            self.note(format!("close_group {}", group.id));
            self.inner.close_group(group)
        }
    }
}
