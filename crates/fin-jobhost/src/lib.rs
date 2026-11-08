//! `fin-jobhost` (5C-1 plan, Decisions 9–23): the host side of the job envelope. It spawns
//! every worker and helper through a [`Spawner`] hook with an explicit environment, places it
//! in its kill domain before it runs, frames the envelope, runs the hello, liveness, stall and
//! timeout timers, and kills whole groups.
//!
//! Threads: one long-lived `jobhost-spawn` thread does every spawn, kill and close (on Linux
//! `PR_SET_PDEATHSIG` follows the thread that forked); one `jobhost-timer` thread; per process
//! a stdout reader, a stderr reader, a writer fed by a bounded queue and a waiter. Events go
//! to an [`EventSink`], which may block: the readers then block, the pipes fill and the
//! worker blocks on its own writes, while the liveness timers do not count that time.
//!
//! Kill domains are per OS ([`domain`]): delegated cgroups on Linux where they are ours,
//! process groups elsewhere on Unix (with the `fin-workers watchdog` for host death), and on
//! Windows a kill-on-close host Job with nested group and leaf Jobs, each process created
//! suspended and resumed only once it is in them.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

pub use fin_jobs::WorkerMsg;
pub use fin_jobs::msg_worker::Hello;

mod commands;
mod domain;
mod exit;
mod framing;
mod host;
mod memory;
mod proc;
pub mod spawner;
#[cfg(feature = "test-hooks")]
mod test_hooks;
#[cfg(test)]
mod tests;
mod timers;
pub mod wire;
mod writer;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;
#[cfg(windows)]
pub use windows::KILLED_EXIT_CODE;

pub use commands::{Done, SpawnDone};
pub use exit::{ExitInfo, ExitReason, KillReason};
pub use host::Host;
pub use memory::MemoryRule;
pub use spawner::{
    JobLimits, PathGrant, PlatformCommand, Profile, Program, Qos, Role, SpawnError, SpawnRequest,
    Spawner, UiLimits, Unsandboxed,
};

/// The host's timers, in milliseconds (Decisions 18, 19, 19a, 20). Tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// From spawn to `hello` (`noHello`).
    pub hello_ms: u32,
    /// Silence after `hello` (`hung`); counted only while the reader waits on its pipe.
    pub hung_ms: u32,
    /// After a kill, how long the host waits for the group's processes to exit.
    pub kill_wait_ms: u32,
    /// The memory sampling interval (process groups: the host checks group limits).
    pub rss_sample_ms: u32,
    /// After an exit, how long stdout may stay silent before `Exit` is sent anyway.
    pub exit_drain_ms: u32,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            hello_ms: 10_000,
            hung_ms: 30_000,
            kill_wait_ms: 5_000,
            rss_sample_ms: 1_000,
            exit_drain_ms: 200,
        }
    }
}

/// Whether delegated cgroups are used where Decision 16's conditions hold (Linux). `Off`, or
/// `FIN_CGROUPS=off` in the environment, means process groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CgroupMode {
    #[default]
    Auto,
    Off,
}

/// A log line from the host (abandoned processes, failed kills). The default writes to stderr.
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// How the host runs.
#[derive(Clone)]
pub struct HostOptions {
    /// Where `Program::Bundled` names resolve (beside the `fin-node` binary, Decision 2).
    pub binary_dir: PathBuf,
    pub timing: Timing,
    pub spawner: Arc<dyn Spawner>,
    pub cgroups: CgroupMode,
    /// Start the Unix host watchdog (`fin-workers watchdog`, Decision 15); ignored on Windows.
    pub watchdog: bool,
    /// The envelope minor this host speaks ([`fin_jobs::MINOR`]); a plugin process above it
    /// is decoded leniently (Decision 3).
    pub host_minor: u32,
    pub log: LogFn,
    /// macOS and Windows: how long a spawned process stays suspended after its placement
    /// (tests).
    #[cfg(feature = "test-hooks")]
    pub resume_delay_for_test: Option<std::time::Duration>,
    /// How long each waiter sleeps before it waits for its process (tests: a process that
    /// looks alive after its kill, so it is abandoned).
    #[cfg(feature = "test-hooks")]
    pub waiter_delay_for_test: Option<std::time::Duration>,
    /// Windows: called with each process while it is suspended in its Jobs; an error fails
    /// the spawn as a failed placement would (tests).
    #[cfg(all(windows, feature = "test-hooks"))]
    pub suspended_hook_for_test: Option<SuspendedHook>,
    /// Windows: the completion port's thread is not started, so only process handles and the
    /// Jobs' own figures report exits (tests).
    #[cfg(all(windows, feature = "test-hooks"))]
    pub port_stopped_for_test: bool,
    /// Windows: the images the spawn broker is made from, in place of the bundled binary
    /// and `cmd.exe` (tests; empty: no broker).
    #[cfg(all(windows, feature = "test-hooks"))]
    pub broker_images_for_test: Option<Vec<PathBuf>>,
    /// Runs after each message `send` queued, before `send` returns (tests: a worker that
    /// answers before `send` returns).
    #[cfg(feature = "test-hooks")]
    pub after_send_for_test: Option<AfterSendHook>,
    /// The waiter thread of each spawn does not start (tests: the spawn thread's fallback).
    #[cfg(feature = "test-hooks")]
    pub fail_waiter_thread_for_test: bool,
    /// [`Host::start`] panics (tests: a caller's answer that is never given).
    #[cfg(feature = "test-hooks")]
    pub panic_on_start_for_test: bool,
    /// Each spawn command panics before it answers (tests: the spawn thread survives, the
    /// dropped callback is the caller's to answer).
    #[cfg(feature = "test-hooks")]
    pub panic_on_spawn_for_test: bool,
}

/// See [`HostOptions::after_send_for_test`].
#[cfg(feature = "test-hooks")]
pub type AfterSendHook = Arc<dyn Fn(ProcId, &fin_jobs::HostMsg) + Send + Sync>;

/// What a Windows test hook learns about a process while it is suspended in its Jobs.
#[cfg(all(windows, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuspendedProbe {
    pub pid: u32,
    /// `IsProcessInJob` for the host, group and leaf Jobs.
    pub in_host: bool,
    pub in_group: bool,
    pub in_leaf: bool,
    /// The primary thread's suspend count (1: created suspended, not resumed).
    pub suspend_count: u32,
}

/// See [`HostOptions::suspended_hook_for_test`].
#[cfg(all(windows, feature = "test-hooks"))]
pub type SuspendedHook = Arc<dyn Fn(&SuspendedProbe) -> std::io::Result<()> + Send + Sync>;

impl HostOptions {
    /// Defaults: [`Timing::default`], [`Unsandboxed`], cgroups `auto`, the watchdog on.
    pub fn new(binary_dir: impl Into<PathBuf>) -> HostOptions {
        HostOptions {
            binary_dir: binary_dir.into(),
            timing: Timing::default(),
            spawner: Arc::new(Unsandboxed),
            cgroups: CgroupMode::Auto,
            watchdog: true,
            host_minor: fin_jobs::MINOR,
            log: Arc::new(|line| eprintln!("fin-jobhost: {line}")),
            #[cfg(feature = "test-hooks")]
            resume_delay_for_test: None,
            #[cfg(feature = "test-hooks")]
            waiter_delay_for_test: None,
            #[cfg(all(windows, feature = "test-hooks"))]
            suspended_hook_for_test: None,
            #[cfg(all(windows, feature = "test-hooks"))]
            port_stopped_for_test: false,
            #[cfg(all(windows, feature = "test-hooks"))]
            broker_images_for_test: None,
            #[cfg(feature = "test-hooks")]
            after_send_for_test: None,
            #[cfg(feature = "test-hooks")]
            fail_waiter_thread_for_test: false,
            #[cfg(feature = "test-hooks")]
            panic_on_start_for_test: false,
            #[cfg(feature = "test-hooks")]
            panic_on_spawn_for_test: false,
        }
    }
}

impl fmt::Debug for HostOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostOptions")
            .field("binary_dir", &self.binary_dir)
            .field("timing", &self.timing)
            .field("spawner", &self.spawner.name())
            .field("cgroups", &self.cgroups)
            .field("watchdog", &self.watchdog)
            .field("host_minor", &self.host_minor)
            .finish_non_exhaustive()
    }
}

/// The kill tree in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillTree {
    JobObject,
    Cgroup,
    ProcessGroup,
}

impl KillTree {
    pub fn as_str(self) -> &'static str {
        match self {
            KillTree::JobObject => "jobObject",
            KillTree::Cgroup => "cgroup",
            KillTree::ProcessGroup => "processGroup",
        }
    }
}

/// What this host can enforce (`workers()`, `server.status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCaps {
    pub kill_tree: KillTree,
    pub watchdog: bool,
    /// Group memory limits are enforced by the kernel (cgroup `memory.max` or a Job).
    pub cgroup_memory: bool,
    /// `JobLimits` are enforced (leaf Jobs or leaf cgroups; not with process groups).
    pub leaf_limits: bool,
    /// The spawner's name (`none` when unsandboxed).
    pub sandbox: &'static str,
    pub sandboxed: bool,
}

/// A process the host started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcId(pub u64);

/// A kill domain: a cold job's processes, or a warm worker, or a warm job's helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(pub u64);

impl fmt::Display for ProcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "p{}", self.0)
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "g{}", self.0)
    }
}

/// A group to open: its label (logs), its memory limit (the sum of its processes, Decision
/// 20) and its QoS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpec {
    pub label: String,
    pub memory_bytes: Option<u64>,
    pub qos: Qos,
}

/// The job timers of a running job (Decision 19): `stall_ms` without `begin`, `item` or
/// `progress`; `timeout_ms` from now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub job: String,
    pub stall_ms: Option<u32>,
    pub timeout_ms: Option<u32>,
}

/// What the host reports. Per process the order is `Spawned`, `Hello`, its messages and
/// `Stderr` lines, then `Exit` (after the drain, Decision 19a); `GroupEmpty` follows the last
/// `Exit` of a group.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    /// The process runs. Sent after its spawn's callback ran and before its readers start,
    /// so no other event of the process comes before it or before the callback.
    Spawned {
        proc: ProcId,
        group: GroupId,
    },
    Hello {
        proc: ProcId,
        hello: Hello,
    },
    /// Any message but `hello` and `alive` (consumed here). `memory` is the worker group's
    /// figure, on `ready` only (Decision 36).
    Message {
        proc: ProcId,
        msg: WorkerMsg,
        memory: Option<u64>,
    },
    /// A stderr line (≤ 4 KiB).
    Stderr {
        proc: ProcId,
        line: String,
    },
    Exit {
        proc: ProcId,
        info: ExitInfo,
    },
    GroupEmpty {
        group: GroupId,
    },
}

/// Where events go. The call may block (bounded queues, Decision 11a): the readers wait.
#[derive(Clone)]
pub struct EventSink(Arc<dyn Fn(HostEvent) + Send + Sync>);

impl EventSink {
    pub fn new(f: impl Fn(HostEvent) + Send + Sync + 'static) -> EventSink {
        EventSink(Arc::new(f))
    }

    /// A bounded channel: sending blocks while `cap` events wait. Events sent after the
    /// receiver is gone are dropped.
    pub fn channel(cap: usize) -> (EventSink, mpsc::Receiver<HostEvent>) {
        let (tx, rx) = mpsc::sync_channel(cap);
        (
            EventSink::new(move |e| {
                let _ = tx.send(e);
            }),
            rx,
        )
    }

    pub(crate) fn send(&self, e: HostEvent) {
        (self.0)(e)
    }
}

/// Why a host call failed. `UnknownGroup`, `GroupClosed` and `ShutDown` are the caller's
/// mistakes (`JOBHOST_STATE`); the others mean no process could be started
/// (`WORKER_UNAVAILABLE`).
#[derive(Debug)]
pub enum HostError {
    UnknownGroup(GroupId),
    /// The group is closing or closed.
    GroupClosed(GroupId),
    /// The host is shutting down.
    ShutDown,
    /// A program that may not be spawned (`Program::Bundled` outside the known names, or
    /// missing).
    Program(String),
    /// The spawner refused the request.
    Spawner(String),
    /// The OS failed the spawn or the placement.
    Spawn(std::io::Error),
    /// The host refused the spawn for a known reason ([`HostError::reason`]): `noBroker`
    /// (Windows: no spawn broker could be made) or `processLimit` (Windows: a Job the host
    /// runs in allows no more processes).
    Unavailable {
        reason: &'static str,
        detail: String,
    },
}

impl HostError {
    pub fn code(&self) -> &'static str {
        match self {
            HostError::UnknownGroup(_) | HostError::GroupClosed(_) | HostError::ShutDown => {
                "JOBHOST_STATE"
            }
            _ => "WORKER_UNAVAILABLE",
        }
    }

    /// The `reason` detail of a `WORKER_UNAVAILABLE` the host can name.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            HostError::Unavailable { reason, .. } => Some(reason),
            _ => None,
        }
    }

    /// A spawn's OS error, or the [`Refusal`] it carries.
    pub(crate) fn from_spawn(e: std::io::Error) -> HostError {
        match e.get_ref().and_then(|x| x.downcast_ref::<Refusal>()) {
            Some(r) => HostError::Unavailable {
                reason: r.reason,
                detail: r.detail.clone(),
            },
            None => HostError::Spawn(e),
        }
    }
}

/// A spawn refused for a named reason, carried inside an `io::Error` up to
/// [`HostError::from_spawn`].
#[derive(Debug)]
pub(crate) struct Refusal {
    pub reason: &'static str,
    pub detail: String,
}

impl Refusal {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn io(reason: &'static str, detail: String) -> std::io::Error {
        std::io::Error::other(Refusal { reason, detail })
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.reason, self.detail)
    }
}

impl std::error::Error for Refusal {}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::UnknownGroup(g) => write!(f, "unknown group {g}"),
            HostError::GroupClosed(g) => write!(f, "group {g} is closed"),
            HostError::ShutDown => write!(f, "the job host is shutting down"),
            HostError::Program(p) => write!(f, "program refused: {p}"),
            HostError::Spawner(e) => write!(f, "spawner refused: {e}"),
            HostError::Spawn(e) => write!(f, "spawn failed: {e}"),
            HostError::Unavailable { reason, detail } => {
                write!(f, "spawn refused ({reason}): {detail}")
            }
        }
    }
}

impl std::error::Error for HostError {}

/// Why [`Host::send`] did not enqueue. Neither `QueueFull` nor `Gone` names how the process
/// ends: that comes with its `Exit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// 256 messages or 4 MiB wait: the process is not reading and is being killed as hung.
    QueueFull,
    /// The process exited, or its stdin is closed.
    Gone,
    /// The message cannot be framed (too long).
    Encode(String),
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::QueueFull => write!(f, "the process's queue is full"),
            SendError::Gone => write!(f, "the process is gone"),
            SendError::Encode(e) => write!(f, "cannot frame the message: {e}"),
        }
    }
}

impl std::error::Error for SendError {}
