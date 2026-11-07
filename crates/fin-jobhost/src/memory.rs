//! Memory figures (Decision 20). A group's figure is its domain's, where the kernel keeps one
//! (cgroup `memory.current`, a Job's accounting), else the sum of its processes' samples.
//!
//! Task 3 has no sampler: [`sample`] answers `None`, so `ready` carries no figure with process
//! groups. Task 4 adds the samples (Linux `RssAnon + RssShmem` from `/proc/<pid>/status`, macOS
//! `ri_phys_footprint`) and the per-group limit check every `rss_sample_ms`.

use crate::domain::{GroupHandle, KillDomain};

/// One process's sampled figure.
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
