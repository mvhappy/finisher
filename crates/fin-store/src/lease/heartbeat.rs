//! The heartbeat thread of a [`super::Lease`].
//!
//! Every [`super::Params::heartbeat`] (monotonic), one beat:
//! 1. re-probe server time when the sample is 30 s old, or the last probe failed or did not
//!    advance (errors are not fatal here: the old sample stays usable up to 60 s);
//! 2. the stamp is server time now ([`super::ServerClock::now_server_ms`]); without one the
//!    beat fails;
//! 3. the fence: `lease.json` must still be what this lease wrote (else lost). The record is
//!    only rewritten after this check, so a deleted `lease.json` is never recreated
//!    (`write_in_place` never creates either);
//! 4. rewrite it in place with `heartbeat + 1` and the stamp;
//! 5. "checked after every heartbeat": read it back, it must be exactly that record.
//!
//! A beat that fails (no server time, a write error) is retried after [`super::RETRY_AFTER`];
//! nothing is lost until the self-fence deadline passes. Evidence of another writer (steps 3
//! and 5, or `lease.json` gone) loses the lease at once. The thread never panics the process:
//! a panic in a beat is caught and loses the lease. It ends when the lease is lost or
//! dropped, and then tells the registry that this process no longer writes the record.

use std::io::ErrorKind;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

use super::clock::lock;
use super::record::{self, LeaseRecord};
use super::{Inner, RETRY_AFTER, registry};
use crate::fsio;

enum Failed {
    /// Try again after [`RETRY_AFTER`].
    Retry,
    Lost(String),
}

pub(super) fn run(inner: &Arc<Inner>) {
    if catch_unwind(AssertUnwindSafe(|| beat_until_stopped(inner))).is_err() {
        inner.set_lost("the heartbeat thread panicked".to_string());
    }
    exited(inner);
}

/// The thread is done: no write of this lease happens any more.
pub(super) fn exited(inner: &Inner) {
    registry::stopped(&inner.key, inner.epoch);
    *lock(&inner.exited) = true;
    inner.exited_cv.notify_all();
}

fn beat_until_stopped(inner: &Inner) {
    let mut next = lock(&inner.status).last_ok + inner.params.heartbeat;
    loop {
        {
            let stop = lock(&inner.stop);
            if *stop {
                return;
            }
            let (stop, _) = inner
                .wake
                .wait_timeout(stop, inner.params.poll)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *stop {
                return;
            }
        }
        if inner.deadline().is_err() {
            return;
        }
        if inner.now() < next {
            continue;
        }
        match beat(inner) {
            Ok(stamped) => next = stamped + inner.params.heartbeat,
            Err(Failed::Retry) => {
                lock(&inner.status).failed += 1;
                next = inner.now() + RETRY_AFTER;
            }
            Err(Failed::Lost(reason)) => {
                inner.set_lost(reason);
                return;
            }
        }
    }
}

/// One beat (module docs). Returns the monotonic instant it stamped.
fn beat(inner: &Inner) -> Result<Instant, Failed> {
    {
        let mut s = lock(&inner.status);
        if s.fail_next > 0 {
            if s.fail_next != u32::MAX {
                s.fail_next -= 1;
            }
            return Err(Failed::Retry);
        }
    }
    let mut io = lock(&inner.io);
    if io.clock.needs_probe(inner.now()) {
        let _ = io.clock.probe();
    }
    let now = inner.now();
    let stamp = io.clock.now_server_ms(now).ok_or(Failed::Retry)?;
    inner.verify(&mut io).map_err(Failed::Lost)?;
    let next = LeaseRecord {
        heartbeat: io.last.heartbeat + 1,
        probe_server_ms: stamp,
        ..io.last.clone()
    };
    io.pending = Some(next.clone());
    let path = inner.path();
    match fsio::write_in_place(&path, &record::encode(&next)) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(Failed::Lost("lease.json is missing".to_string()));
        }
        Err(_) => return Err(Failed::Retry),
    }
    match record::read(&path) {
        Ok(record::Seen::Valid(r)) if r == next => {}
        Ok(record::Seen::Valid(r)) => {
            return Err(Failed::Lost(format!(
                "lease.json names server {} at epoch {} right after our heartbeat",
                r.server_id, r.epoch
            )));
        }
        Ok(_) => {
            return Err(Failed::Lost(
                "lease.json did not read back after our heartbeat".to_string(),
            ));
        }
        Err(e) => return Err(Failed::Lost(format!("lease.json is unreadable: {e}"))),
    }
    io.pending = None;
    registry::wrote(&inner.key, &next);
    io.last = next;
    drop(io);
    let mut s = lock(&inner.status);
    if now > s.last_ok {
        s.last_ok = now;
    }
    s.heartbeats += 1;
    Ok(now)
}
