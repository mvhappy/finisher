//! The sweep (plan 7C Decision 18): every grant whose owner is not live is revoked, and every
//! revoke a failure left pending is retried. `finisherd` sweeps at start, before any listener,
//! with no owner live; the static owner is always live. A revoke that cannot be done does not
//! stop the sweep: it stays pending for the next one and is answered in [`SweepReport::failed`].

use std::collections::HashSet;

use super::wal::Record;
use super::{Apply, Broker, STATIC_OWNER};

/// What a sweep did.
#[derive(Debug, Default)]
pub struct SweepReport {
    /// Grants of owners not live, revoked.
    pub revoked: usize,
    /// The revokes that failed (each left pending), as messages.
    pub failed: Vec<String>,
}

impl<A: Apply> Broker<A> {
    /// Revokes the grants of every owner not in `live` and retries pending revokes.
    pub fn sweep(&self, live: &HashSet<String>) -> SweepReport {
        let mut report = SweepReport::default();
        for r in self.pending_revokes() {
            if let Record::Revoke { id, path, sid, .. } = r {
                let _path = self.take_path(&path);
                // Another thread may have finished it while this one waited for the path.
                if !super::lock(&self.state).pending.contains_key(&id) {
                    continue;
                }
                if let Err(e) = self.complete_revoke(id, &path, &sid) {
                    report.failed.push(e.to_string());
                }
            }
        }
        let gone: Vec<_> = self
            .grants()
            .into_iter()
            .filter(|g| g.owner != STATIC_OWNER && !live.contains(&g.owner))
            .collect();
        for g in &gone {
            match self.revoke_one(&g.path, &g.sid, &g.owner) {
                Ok(()) => report.revoked += 1,
                Err(e) => report.failed.push(e.to_string()),
            }
        }
        report
    }

    fn pending_revokes(&self) -> Vec<Record> {
        super::lock(&self.state)
            .pending
            .values()
            .filter(|r| matches!(r, Record::Revoke { .. }))
            .cloned()
            .collect()
    }
}
