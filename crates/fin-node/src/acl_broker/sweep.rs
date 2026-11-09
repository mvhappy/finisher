//! The sweep (plan 7C Decision 18): every grant whose owner is not live is revoked, and every
//! revoke a failure left pending is retried. `finisherd` sweeps at start, before any listener,
//! with no owner live; the static owner is always live.

use std::collections::HashSet;

use super::wal::Record;
use super::{Apply, Broker, BrokerError, STATIC_OWNER};

impl<A: Apply> Broker<A> {
    /// Revokes the grants of every owner not in `live` (and retries pending revokes); how
    /// many grants it revoked.
    pub fn sweep(&self, live: &HashSet<String>) -> Result<usize, BrokerError> {
        let stale: Vec<Record> = self.pending_revokes();
        for r in stale {
            if let Record::Revoke { id, path, sid, .. } = r {
                let _path = self.take_path(&path);
                // Another thread may have finished it while this one waited for the path.
                if !super::lock(&self.state).pending.contains_key(&id) {
                    continue;
                }
                self.apply_revoke(&path, &sid)?;
                self.done(id, None)?;
            }
        }
        let gone: Vec<_> = self
            .grants()
            .into_iter()
            .filter(|g| g.owner != STATIC_OWNER && !live.contains(&g.owner))
            .collect();
        for g in &gone {
            self.revoke_one(&g.path, &g.sid, &g.owner)?;
        }
        Ok(gone.len())
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
