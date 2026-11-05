//! [`Contender::take`]: claiming a fresh epoch and writing our record (module docs of
//! [`super`], "Taking").

use std::io::ErrorKind;
use std::path::Path;

use super::super::record::{self, LeaseRecord, Seen};
use super::super::registry;
use super::super::{Lease, LeaseState, Previous, claim, server_lock};
use super::publish::{create_record, remove_temps};
use super::{Contender, Expected};
use crate::fsio;

impl Contender {
    /// Claims a fresh epoch and writes our record over `expected` (module docs).
    pub(super) fn take(&mut self, expected: Expected) -> Result<Lease, LeaseState> {
        self.evidence = None;
        let leftover = std::mem::take(&mut self.leftover);
        let own_claim = self.own_claim.take();
        // 0. The server-data lock. Busy: another process of this server data holds a lease
        // here, unless the record is one this process wrote (then it is our own lost lease's
        // thread, still stuck in I/O, that keeps it).
        let mut held = self.locked.take();
        if held.is_none()
            && let Some(cfg) = &self.server_lock
        {
            match server_lock::try_lock(&cfg.path) {
                server_lock::Attempt::Locked(h) => held = Some(h),
                server_lock::Attempt::Busy if !leftover => {
                    return Err(LeaseState::HeldElsewhere {
                        holder: "this server, in another process".to_string(),
                        epoch: 0,
                        age_server_ms: None,
                    });
                }
                _ => {}
            }
        }
        let mono = self.params.clock.clone();
        let lost = |reason: String| LeaseState::Lost { reason };
        let busy = |holder: String, epoch| LeaseState::HeldElsewhere {
            holder,
            epoch,
            age_server_ms: None,
        };
        let claims = claim::list(&self.dir)
            .map_err(|e| lost(format!("cannot list {}: {e}", self.dir.display())))?;
        let floor = match &expected {
            Expected::Record(r) => r.epoch,
            Expected::Missing | Expected::Corrupt(_) => 0,
        };
        // 1. Another acquisition in progress.
        let server_now = self
            .counted
            .and_then(|p| super::super::clock::counted_server_ms(&p, mono.now()));
        let stale_ms = i64::try_from(self.params.stale_after.as_millis()).unwrap_or(i64::MAX);
        let in_progress = |c: &claim::ClaimFile| match (server_now, c.server_ms) {
            (Some(now), Some(m)) => now - m <= stale_ms,
            _ => true,
        };
        if let Some(rival) = claims
            .iter()
            .rev()
            .find(|c| c.epoch > floor && Some(c.epoch) != own_claim && in_progress(c))
        {
            let who = claim::owner(&self.dir, rival.epoch)
                .unwrap_or_else(|| "another server (acquiring)".to_string());
            return Err(busy(who, rival.epoch));
        }
        // 2. Claim the one epoch after everything ever used here.
        let base = [
            floor,
            claims.last().map_or(0, |c| c.epoch),
            claim::highest_segment_epoch(&self.dir),
            registry::last_epoch(&registry::key(&self.dir, &self.server_id)).unwrap_or(0),
        ]
        .into_iter()
        .max()
        .unwrap_or(0);
        let claimed = match claim::claim_next(&self.dir, base, &self.server_id) {
            Ok(c) => c,
            // Someone else claimed it from the same view: back off, creating nothing. (Or an
            // earlier attempt of ours landed although it reported a transient error: then the
            // claim names us, and it ages out like any orphan claim.)
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let who = match claim::owner(&self.dir, base + 1) {
                    Some(id) if id == self.server_id => {
                        "this server (a claim left by its own retried attempt)".to_string()
                    }
                    _ => "another server (acquiring)".to_string(),
                };
                return Err(busy(who, base + 1));
            }
            Err(e) => return Err(lost(format!("cannot create a lease claim: {e}"))),
        };
        let epoch = claimed.epoch;
        let back_off = |state: LeaseState| {
            claim::remove(&self.dir, epoch);
            Err(state)
        };
        let stamped = mono.now();
        let Some(stamp) = self.clock.now_server_ms(stamped) else {
            return back_off(lost("no server time: the probe file cannot be read".into()));
        };
        let mine = LeaseRecord::new(&self.server_id, epoch, 0, &self.holder, stamp);
        let bytes = record::encode(&mine);
        let path = self.record_path();
        // 3. Unchanged since judged? 4. Then write.
        let wrote = match (&expected, record::read(&path)) {
            (Expected::Missing, Ok(Seen::Missing)) => create_record(&self.dir, &path, &bytes),
            (Expected::Record(r), Ok(Seen::Valid(now))) if now == *r => {
                fsio::write_in_place(&path, &bytes)
            }
            (Expected::Corrupt(b), Ok(Seen::Corrupt(now))) if now == *b => {
                fsio::write_in_place(&path, &bytes)
            }
            (_, Ok(seen)) => return back_off(changed(seen)),
            (_, Err(e)) => {
                return back_off(busy(format!("unknown (lease.json is unreadable: {e})"), 0));
            }
        };
        match wrote {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                return back_off(busy("another server".into(), epoch));
            }
            Err(e) => {
                // An in-place write that failed may have landed in part. Put back what was
                // there, so the folder is as it was judged (a torn record would read as
                // damaged to everyone, and damaged records need a confirmed takeover).
                // A record we were creating is not in place when `create_record` fails (a
                // publish that landed despite an error is recognised there as success).
                restore(&path, &expected);
                return back_off(lost(format!("cannot write lease.json: {e}")));
            }
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(f) = self.after_write.take() {
            f(&self.dir);
        }
        // 5. Read back. Anything but our record: remove our claim (a young orphan claim
        // would hold others off for 45 s). A torn read-back, while no claim above ours exists,
        // is our write landing badly: rewrite it once rather than leave a damaged record,
        // which would need a confirmed takeover. Another server cannot be writing it then:
        // it would hold a claim above ours.
        let mut seen = record::read(&path);
        if matches!(seen, Ok(Seen::Corrupt(_)))
            && claim::list(&self.dir).is_ok_and(|c| c.iter().all(|c| c.epoch <= epoch))
            && fsio::write_in_place(&path, &bytes).is_ok()
        {
            seen = record::read(&path);
        }
        match seen {
            Ok(Seen::Valid(r)) if r == mine => {}
            Ok(seen) => return back_off(changed(seen)),
            Err(e) => return back_off(lost(format!("lease.json is unreadable: {e}"))),
        }
        // 6. What this acquisition replaced, for the owner's ordering rule (module docs of
        // [`super`]), before the claims below ours are gone.
        let previous = Previous {
            record: match &expected {
                Expected::Record(r) => Some(r.server_id.clone()),
                Expected::Missing | Expected::Corrupt(_) => None,
            },
            damaged: matches!(expected, Expected::Corrupt(_)),
            claims: claims
                .iter()
                .filter(|c| c.epoch < epoch)
                .map(|c| (c.epoch, claim::owner(&self.dir, c.epoch)))
                .collect(),
        };
        claim::remove_below(&self.dir, epoch);
        remove_temps(&self.dir);
        Ok(Lease::start(
            &self.dir,
            registry::key(&self.dir, &self.server_id),
            self.params.clone(),
            self.clock.clone(),
            mine,
            stamped,
            previous,
            held,
        ))
    }
}

/// Best effort: `lease.json` back to what it was before our failed in-place write.
fn restore(path: &Path, expected: &Expected) {
    let _ = match expected {
        Expected::Record(r) => fsio::write_in_place(path, &record::encode(r)),
        Expected::Corrupt(b) => fsio::write_in_place(path, b),
        Expected::Missing => Ok(()),
    };
}

/// `lease.json` changed between judging and writing.
fn changed(seen: Seen) -> LeaseState {
    let (holder, epoch) = match seen {
        Seen::Valid(r) => (r.holder, r.epoch),
        Seen::Missing => ("unknown (lease.json is missing)".to_string(), 0),
        Seen::Corrupt(_) => ("unknown (lease.json is damaged)".to_string(), 0),
    };
    LeaseState::HeldElsewhere {
        holder,
        epoch,
        age_server_ms: None,
    }
}
