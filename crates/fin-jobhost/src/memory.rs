//! Memory figures and limits (Decision 20). A group's figure is its domain's, where the kernel
//! keeps one (cgroup `memory.current`, a Job's accounting), else the sum of its processes'
//! samples (Linux `RssAnon + RssShmem`, macOS `ri_phys_footprint`; `crate::os::memory`).
//!
//! Where the kernel does not enforce a group's limit (process groups), the host samples every
//! limited group each `rss_sample_ms` and kills it when the sum passes the limit: every member
//! is reported `Oom`, with the figure seen as `memory_peak`.

use std::sync::{Arc, Weak};
use std::time::Instant;

use crate::domain::{GroupHandle, KillDomain};
use crate::exit::ExitReason;
use crate::host::{Inner, ms_};
use crate::{GroupId, ProcId};

/// One process's sampled figure.
#[cfg(unix)]
pub(crate) fn sample(pid: u32) -> Option<u64> {
    crate::os::memory::sample(pid)
}

#[cfg(not(unix))]
pub(crate) fn sample(_pid: u32) -> Option<u64> {
    None
}

/// The group's figure: the domain's, else the sum of `pids`' samples (`None` if any is
/// missing).
pub(crate) fn group_figure(
    domain: &dyn KillDomain,
    group: &GroupHandle,
    pids: &[u32],
) -> Option<u64> {
    domain
        .memory(group)
        .or_else(|| pids.iter().map(|&p| sample(p)).sum())
}

impl Inner {
    /// The figure attached to `ready`.
    pub fn group_memory(&self, group: GroupId) -> Option<u64> {
        let (handle, pids) = {
            let t = self.tables();
            let g = t.groups.get(&group)?;
            let pids: Vec<u32> = g
                .live
                .iter()
                .filter_map(|p| t.procs.get(p))
                .map(|p| p.pid)
                .collect();
            (g.handle.clone()?, pids)
        };
        crate::memory::group_figure(&*self.domain, &handle, &pids)
    }
}

/// Starts the sampler, unless the kernel enforces the limits.
pub(crate) fn start_sampler(inner: &Arc<Inner>) {
    if !inner.domain.kernel_memory() && cfg!(unix) {
        arm(Arc::downgrade(inner), inner, Instant::now());
    }
}

fn arm(w: Weak<Inner>, inner: &Inner, from: Instant) {
    let next = from + ms_(inner.opts.timing.rss_sample_ms.max(10));
    inner.timers.at(next, move || {
        if let Some(i) = w.upgrade() {
            check(&i);
            arm(w, &i, next.max(Instant::now()));
        }
    });
}

/// A limited group's id, limit, handle and live `(proc, pid)`s.
type Limited = (GroupId, u64, Arc<GroupHandle>, Vec<(ProcId, u32)>);

/// One round: every live group with a limit.
fn check(inner: &Inner) {
    let limited: Vec<Limited> = {
        let t = inner.tables();
        t.groups
            .iter()
            .filter_map(|(id, g)| {
                let limit = g.spec.memory_bytes?;
                let procs: Vec<(ProcId, u32)> = g
                    .live
                    .iter()
                    .filter_map(|p| t.procs.get(p).map(|p| (p.id, p.pid)))
                    .collect();
                let handle = g.handle.clone()?;
                (!procs.is_empty()).then_some((*id, limit, handle, procs))
            })
            .collect()
    };
    for (gid, limit, handle, procs) in limited {
        // A member that exited since the snapshot counts 0.
        let used: u64 = procs.iter().filter_map(|&(_, pid)| sample(pid)).sum();
        handle.note_sample(used);
        if used > limit {
            inner.log(&format!(
                "{gid}: {used} bytes over its limit of {limit}; killing it"
            ));
            inner.kill_for(procs[0].0, ExitReason::Oom);
        }
    }
}

/// A group's memory rule (Decision 20; the family's per class, or a kind's override). It
/// resolves at dispatch to bytes: `fixed_mib`, else `min(cap_mib, ram_fraction × total)`
/// (either bound alone applies by itself); `None` without any.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MemoryRule {
    pub fixed_mib: Option<u64>,
    pub ram_fraction: Option<f64>,
    pub cap_mib: Option<u64>,
}

impl MemoryRule {
    /// The limit in bytes for a machine with `total_ram` bytes.
    pub fn resolve(&self, total_ram: u64) -> Option<u64> {
        const MIB: u64 = 1 << 20;
        if let Some(f) = self.fixed_mib {
            return Some(f.saturating_mul(MIB));
        }
        let frac = self
            .ram_fraction
            .filter(|f| f.is_finite() && *f > 0.0)
            .map(|f| (total_ram as f64 * f.min(1.0)) as u64);
        let cap = self.cap_mib.map(|c| c.saturating_mul(MIB));
        match (frac, cap) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}
