//! Export cadence, the lease seen by export, and the temp-file sweep.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fin_core::CoreError;

use crate::names;

/// Segment export delay after commits…
pub(crate) const DELAY: Duration = Duration::from_secs(2);
/// …and under churn: more than [`CHURN_SEGMENTS`] segments within [`CHURN_WINDOW`].
pub(crate) const CHURN_DELAY: Duration = Duration::from_secs(30);
pub(crate) const CHURN_SEGMENTS: usize = 30;
pub(crate) const CHURN_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Temp files unchanged this long (2×TTL) are crash leftovers.
pub(crate) const TEMP_MAX_AGE: Duration = Duration::from_secs(20);
/// How often the temp sweep runs.
pub(crate) const SWEEP_EVERY: Duration = Duration::from_secs(10);

/// How long to wait before writing: 2 s, or 30 s while more than 30 segments were written in
/// the last 10 minutes.
#[derive(Debug, Default)]
pub(crate) struct ExportPolicy {
    recent: VecDeque<Instant>,
}

impl ExportPolicy {
    pub(crate) fn delay(&self, now: Instant) -> Duration {
        let recent = self
            .recent
            .iter()
            .filter(|t| now.saturating_duration_since(**t) <= CHURN_WINDOW)
            .count();
        if recent > CHURN_SEGMENTS {
            CHURN_DELAY
        } else {
            DELAY
        }
    }

    pub(super) fn wrote(&mut self, now: Instant) {
        self.recent.push_back(now);
        while self
            .recent
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) > CHURN_WINDOW)
        {
            self.recent.pop_front();
        }
    }
}

/// The lease as export sees it. Task 9 adds the real `Lease` (whose `check()` returns the
/// epoch or `LeaseLost`); until then a store always holds epoch 0.
#[derive(Debug, Default)]
pub(crate) enum ExportLease {
    #[default]
    NoLease,
    /// Test hook: a lease at `epoch`, held or lost.
    #[cfg(any(test, feature = "test-hooks"))]
    Fixed { epoch: u64, held: bool },
}

impl ExportLease {
    /// The current epoch if this server may write, else why not.
    pub(crate) fn check(&self) -> Result<u64, CoreError> {
        match self {
            ExportLease::NoLease => Ok(0),
            #[cfg(any(test, feature = "test-hooks"))]
            ExportLease::Fixed { epoch, held: true } => Ok(*epoch),
            #[cfg(any(test, feature = "test-hooks"))]
            ExportLease::Fixed { held: false, .. } => {
                Err(CoreError::new("LEASE_LOST", "the lease is lost"))
            }
        }
    }

    /// The epoch a commit made now is recorded at (`txs.epoch`).
    pub(crate) fn epoch(&self) -> u64 {
        match self {
            ExportLease::NoLease => 0,
            #[cfg(any(test, feature = "test-hooks"))]
            ExportLease::Fixed { epoch, .. } => *epoch,
        }
    }
}

/// Removes our own temp files once they have been seen unchanged for [`TEMP_MAX_AGE`].
///
/// Age is observed, never computed from the mtime: the mtime is the file server's clock,
/// which may be far from ours. Each temp file is remembered with its `(len, mtime)` and the
/// monotonic time it was first seen so; any change restarts its clock.
#[derive(Debug, Default)]
pub(crate) struct TempSweep {
    seen: HashMap<PathBuf, (u64, Option<SystemTime>, Instant)>,
}

impl TempSweep {
    /// One sweep of `root/*`. Errors are ignored: the next sweep tries again.
    pub(super) fn sweep(&mut self, root: &Path, now: Instant) {
        let mut present = HashMap::new();
        for dir in fs::read_dir(root).into_iter().flatten().flatten() {
            let Ok(entries) = fs::read_dir(dir.path()) else {
                continue;
            };
            for e in entries.flatten() {
                if !e.file_name().to_str().is_some_and(names::is_temp) {
                    continue;
                }
                let Ok(meta) = e.metadata() else {
                    continue;
                };
                let state = (meta.len(), meta.modified().ok());
                let path = e.path();
                let since = match self.seen.get(&path) {
                    Some((len, mtime, since)) if (*len, *mtime) == state => *since,
                    _ => now,
                };
                if now.saturating_duration_since(since) >= TEMP_MAX_AGE
                    && fs::remove_file(&path).is_ok()
                {
                    continue;
                }
                present.insert(path, (state.0, state.1, since));
            }
        }
        self.seen = present;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_is_2s_then_30s_above_30_segments_in_10_minutes() {
        let t0 = Instant::now();
        let mut p = ExportPolicy::default();
        assert_eq!(p.delay(t0), DELAY);
        for i in 0..30 {
            p.wrote(t0 + Duration::from_secs(i));
        }
        assert_eq!(p.delay(t0 + Duration::from_secs(30)), DELAY);
        p.wrote(t0 + Duration::from_secs(30));
        assert_eq!(p.delay(t0 + Duration::from_secs(31)), CHURN_DELAY);
        // The oldest leaves the window 10 minutes after it was written.
        assert_eq!(p.delay(t0 + CHURN_WINDOW), CHURN_DELAY);
        assert_eq!(p.delay(t0 + CHURN_WINDOW + Duration::from_secs(1)), DELAY);
        p.wrote(t0 + CHURN_WINDOW + Duration::from_secs(100));
        assert!(p.recent.len() < 31, "old entries are pruned");
    }
}
