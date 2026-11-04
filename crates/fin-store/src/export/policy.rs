//! Export cadence, the lease seen by export, and the temp-file sweep.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use fin_core::CoreError;

use crate::names;

/// Segment export delay after commits…
pub(crate) const DELAY: Duration = Duration::from_secs(2);
/// …and under churn: more than [`CHURN_SEGMENTS`] segments within [`CHURN_WINDOW`].
pub(crate) const CHURN_DELAY: Duration = Duration::from_secs(30);
pub(crate) const CHURN_SEGMENTS: usize = 30;
pub(crate) const CHURN_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Temp files older than this (2×TTL) are crash leftovers.
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
}

impl ExportLease {
    /// The epoch to write at, or why this server may not write.
    pub(crate) fn check(&self) -> Result<u64, CoreError> {
        match self {
            ExportLease::NoLease => Ok(0),
        }
    }
}

/// Removes our own temp files older than [`TEMP_MAX_AGE`] in `root/*`. Errors are ignored:
/// the next sweep tries again.
pub(super) fn sweep_temps(root: &Path) {
    let Ok(branches) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for dir in branches.flatten() {
        let Ok(entries) = fs::read_dir(dir.path()) else {
            continue;
        };
        for e in entries.flatten() {
            let is_ours = e.file_name().to_str().is_some_and(names::is_temp);
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > TEMP_MAX_AGE));
            if is_ours && old {
                let _ = fs::remove_file(e.path());
            }
        }
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
