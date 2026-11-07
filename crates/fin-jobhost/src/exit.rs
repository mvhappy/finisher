//! How a process ended (`HostEvent::Exit`), and the stderr tail kept for it.

use std::collections::VecDeque;

use crate::GroupId;
use crate::host::Inner;
use crate::proc::Proc;

/// Why the host killed a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KillReason {
    Cancel,
    Shutdown,
    Recycle,
    IdleExpired,
    HelperFailed,
    JobEnded,
    Hung,
    Test,
}

/// Why a process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    /// It exited on its own (any code or signal); the engine classifies it.
    Exited,
    /// The host killed its group (`kill_group`, or another member's timer or violation).
    Killed(KillReason),
    /// Silent for `hung_ms` after `hello`, or its stdin queue filled.
    Hung,
    /// No `begin`, `item` or `progress` for the watch's `stall_ms`.
    Stalled,
    /// The watch's `timeout_ms` passed.
    TimedOut,
    /// Over its group's memory limit: the kernel's (cgroup, Job) or the host's sampler.
    Oom,
    /// The kernel reported a resource fault (Task 4).
    ResourceFault,
    /// A framing or protocol violation (`json`, `seq`, `long`, `otherJob`, `unknownType`,
    /// `order`, `hello`, `role`, `selfSandbox`, …).
    ProtocolError(String),
    /// `hello` names another protocol or envelope (Decision 3).
    ProtocolMismatch { expected: String, got: String },
    /// No `hello` within `hello_ms`.
    NoHello,
    /// The process could not start as asked (`selfSandbox`: exit 78 before `hello`).
    SpawnFailed(String),
    /// Still alive after the kill wait: reported once, reaped in the background (Decision 18).
    Abandoned,
}

impl ExitReason {
    /// The reason other members of a group get when one member's timer or violation kills
    /// the group.
    pub(crate) fn for_others(&self) -> ExitReason {
        match self {
            ExitReason::Killed(r) => ExitReason::Killed(*r),
            ExitReason::Hung => ExitReason::Killed(KillReason::Hung),
            // Every member of a group over its limit answers `WORKER_OOM` (Decision 20).
            ExitReason::Oom => ExitReason::Oom,
            _ => ExitReason::Killed(KillReason::JobEnded),
        }
    }
}

/// `HostEvent::Exit`'s facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub reason: ExitReason,
    /// The exit code (Windows: the NTSTATUS or code).
    pub code: Option<i64>,
    /// The signal that ended it, lower case without `SIG` (fin-core's `SIGNALS`; `other` for
    /// one outside it).
    pub signal: Option<&'static str>,
    /// Processes in the group the host did not start (Decision 22; Task 4).
    pub foreign: Option<u32>,
    pub memory_peak: Option<u64>,
    /// The last 8 KiB of stderr.
    pub stderr_tail: String,
    pub lifetime_ms: u64,
}

/// What the OS said about an exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RawExit {
    pub code: Option<i64>,
    pub signal: Option<i32>,
}

/// A Unix signal's name as in fin-core's `SIGNALS`.
#[cfg(unix)]
pub(crate) fn signal_name(sig: i32) -> &'static str {
    match sig {
        libc::SIGKILL => "kill",
        libc::SIGTERM => "term",
        libc::SIGINT => "int",
        libc::SIGHUP => "hup",
        libc::SIGSEGV => "segv",
        libc::SIGBUS => "bus",
        libc::SIGABRT => "abrt",
        libc::SIGILL => "ill",
        libc::SIGFPE => "fpe",
        libc::SIGPIPE => "pipe",
        libc::SIGXCPU => "xcpu",
        libc::SIGSYS => "sys",
        libc::SIGTRAP => "trap",
        _ => "other",
    }
}

#[cfg(not(unix))]
pub(crate) fn signal_name(_sig: i32) -> &'static str {
    "other"
}

/// The longest stderr line forwarded; the rest of a longer line is dropped.
pub(crate) const MAX_STDERR_LINE: usize = 4 << 10;
/// The stderr tail kept for `ExitInfo`.
pub(crate) const STDERR_TAIL: usize = 8 << 10;

/// The last [`STDERR_TAIL`] bytes of a stream.
#[derive(Default)]
pub(crate) struct Tail {
    bytes: VecDeque<u8>,
}

impl Tail {
    pub fn push(&mut self, chunk: &[u8]) {
        let chunk = &chunk[chunk.len().saturating_sub(STDERR_TAIL)..];
        let over = (self.bytes.len() + chunk.len()).saturating_sub(STDERR_TAIL);
        self.bytes.drain(..over);
        self.bytes.extend(chunk);
    }

    /// The tail as text; a character cut at the front is dropped.
    pub fn text(&self) -> String {
        let (a, b) = self.bytes.as_slices();
        let mut v = Vec::with_capacity(a.len() + b.len());
        v.extend_from_slice(a);
        v.extend_from_slice(b);
        let start = v
            .iter()
            .position(|&c| (c & 0xC0) != 0x80)
            .unwrap_or(v.len());
        String::from_utf8_lossy(&v[start..]).into_owned()
    }
}

impl Inner {
    pub fn exit_info(&self, p: &Proc, raw: RawExit) -> ExitInfo {
        let (handle, spawned) = {
            let t = self.tables();
            match t.groups.get(&p.group) {
                Some(g) => (g.handle.clone(), g.spawned),
                None => (None, 0),
            }
        };
        let (facts, foreign, sampled) = match &handle {
            Some(h) => (
                self.domain.exit_facts(h),
                self.domain.foreign(h, spawned),
                h.sampled_peak(),
            ),
            None => (Default::default(), None, None),
        };
        let mut info = bare_exit_info(p, raw);
        if facts.oom && matches!(info.reason, ExitReason::Exited) {
            info.reason = ExitReason::Oom;
        }
        info.foreign = foreign;
        info.memory_peak = facts.memory_peak.or(sampled);
        info
    }

    /// Counts the group's foreign processes now (at each `result`, Decision 22), so one that
    /// is gone by the exit is still counted.
    pub fn scan_foreign(&self, group: GroupId) {
        let (handle, spawned) = {
            let t = self.tables();
            let Some(g) = t.groups.get(&group) else {
                return;
            };
            (g.handle.clone(), g.spawned)
        };
        if let Some(h) = handle {
            self.domain.foreign(&h, spawned);
        }
    }
}

/// `ExitInfo` from the process alone (also when the host is gone).
pub(crate) fn bare_exit_info(p: &Proc, raw: RawExit) -> ExitInfo {
    let st = p.state();
    let reason = match &st.kill {
        Some(r) => r.clone(),
        None if !st.hello
            && p.self_sandbox.is_some()
            && raw.code == Some(i64::from(fin_jobs::EXIT_SELF_SANDBOX)) =>
        {
            ExitReason::SpawnFailed("selfSandbox".into())
        }
        None => ExitReason::Exited,
    };
    ExitInfo {
        reason,
        code: raw.code,
        signal: raw.signal.map(signal_name),
        foreign: None,
        memory_peak: None,
        stderr_tail: p.stderr_tail(),
        lifetime_ms: p.started.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_8_kib() {
        let mut t = Tail::default();
        t.push(&[b'a'; 6000]);
        t.push(&[b'b'; 6000]);
        let text = t.text();
        assert_eq!(text.len(), STDERR_TAIL);
        assert!(text.ends_with(&"b".repeat(6000)));
        t.push(&vec![b'c'; 20_000]);
        assert_eq!(t.text(), "c".repeat(STDERR_TAIL));
    }

    #[test]
    fn tail_drops_a_cut_character() {
        let mut t = Tail::default();
        t.push("é".repeat(STDERR_TAIL).as_bytes());
        assert!(t.text().chars().all(|c| c == 'é'));
    }
}
