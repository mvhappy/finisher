//! The heartbeat thread of a [`super::Lease`].
//!
//! Every [`super::Params::heartbeat`] (monotonic, counting sleep), one beat:
//! 1. re-probe server time when the sample is 30 s old, or the last probe failed or was not
//!    accepted (errors are not fatal here: the old sample stays usable up to 60 s);
//! 2. the stamp is server time now ([`super::ServerClock::now_server_ms`]); without one the
//!    beat fails;
//! 3. the fence ([`super::fence`]): `lease.json` must still name our server id and epoch, and
//!    no claim above our epoch younger than 45 s of server time may exist (else lost). A read
//!    that fails, a record that does not read as one valid record even after re-reading (a
//!    torn SMB read), or a listing that fails is a failed beat. The record is only rewritten
//!    after this passed, so a deleted `lease.json` is never recreated (`write_in_place` never
//!    creates either), and nothing is written over a record we could not read;
//! 4. right before writing, in memory: the lease was not dropped, not lost, and the
//!    self-fence deadline has not passed. Steps 1–3 can stall for any time (a hung share, a
//!    machine asleep), and a stalled beat must not write as holder after it should have
//!    fenced, or after a takeover it slept through;
//! 5. rewrite it in place with `heartbeat + 1` and the stamp;
//! 6. "checked after every heartbeat": read it back (our id and epoch, and at least our new
//!    heartbeat; failing, torn or an older heartbeat of ours is a failed beat, missing or
//!    another record is lost) and list claims again; then
//!    the deadline once more, measured from the *previous* success: a beat that finished past
//!    it revives nothing.
//!
//! A beat that fails (no server time, a read, write or listing error, a torn record, a write
//! that did not read back) is retried after [`super::RETRY_AFTER`] and counts as no success:
//! the self-fence deadline still runs from the last beat that passed every step. Every such
//! failure that touches the record or the claims (all but a missing server time) also stops
//! a commit from trusting the last full check ([`Inner::doubt`]). Nothing is lost until the deadline passes. Evidence of
//! another writer (a valid record naming another server or epoch, a newer claim) or a
//! deleted `lease.json` loses the lease at once. The thread never panics the process: a panic
//! in a beat is caught and loses the lease. It ends when the lease is lost or dropped, and
//! then tells the registry that this process no longer writes the record.

use std::io::ErrorKind;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

use super::clock::lock;
use super::record::{self, LeaseRecord};
use super::{Fence, Inner, RETRY_AFTER, registry};
use crate::fsio;

enum Failed {
    /// Try again after [`RETRY_AFTER`].
    Retry,
    Lost(String),
    /// The lease was dropped while the beat ran.
    Stopped,
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
            Err(Failed::Stopped) => return,
        }
    }
}

/// Evidence loses the lease; a look that could not tell is a failed beat, retried (the
/// self-fence deadline still bounds it).
fn fence_failed(inner: &Inner, f: Fence) -> Failed {
    match f {
        Fence::Lost(reason) => Failed::Lost(reason),
        Fence::Unsure(_) => {
            inner.doubt();
            Failed::Retry
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
    let fenced = |f| fence_failed(inner, f);
    inner.verify(&mut io).map_err(fenced)?;
    inner.no_newer_claim(Some(stamp)).map_err(fenced)?;
    #[cfg(any(test, feature = "test-hooks"))]
    inner.hang.at(super::HangPoint::BeatBeforeWrite);
    // The probe, the read and the listing may each have stalled (a hung share, a machine
    // asleep): look again, in memory, right before writing.
    if *lock(&inner.stop) {
        return Err(Failed::Stopped);
    }
    inner.deadline().map_err(|l| Failed::Lost(l.reason))?;
    let next = LeaseRecord {
        heartbeat: io.last.heartbeat + 1,
        probe_server_ms: stamp,
        ..io.last.clone()
    };
    let path = inner.path();
    // `.finisher` replaced by a link: the lease there is not this folder's. Any other failure
    // of the check (a share dropping for a moment) is a failed beat, as a failed write is.
    if let Err(e) = crate::reserved::no_links(path.parent().unwrap_or(&path)) {
        if crate::reserved::is_linked(&e) {
            return Err(Failed::Lost(e.to_string()));
        }
        inner.doubt();
        return Err(Failed::Retry);
    }
    match fsio::write_in_place(&path, &record::encode(&next)) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(Failed::Lost("lease.json is missing".to_string()));
        }
        // It may have landed anyway (the next fence adopts it: same server id and epoch), or
        // in part, leaving a torn record: the next commit must not trust the last check.
        Err(_) => {
            inner.doubt();
            return Err(Failed::Retry);
        }
    }
    // A failed or torn read-back cannot tell whether our write landed, and an older heartbeat
    // of ours says it did not (or not yet visibly): a failed beat, not a success that would
    // move the deadline. If it did land, the next beat's fence adopts it (same server id and
    // epoch).
    match record::read_settled(&path) {
        Ok(record::Seen::Valid(r))
            if r.server_id == next.server_id
                && r.epoch == next.epoch
                && r.heartbeat >= next.heartbeat => {}
        Ok(record::Seen::Valid(r)) if r.server_id == next.server_id && r.epoch == next.epoch => {
            inner.doubt();
            return Err(Failed::Retry);
        }
        Ok(record::Seen::Valid(r)) => {
            return Err(Failed::Lost(format!(
                "lease.json names server {} at epoch {} right after our heartbeat",
                r.server_id, r.epoch
            )));
        }
        Ok(record::Seen::Missing) => {
            return Err(Failed::Lost(
                "lease.json is missing right after our heartbeat".to_string(),
            ));
        }
        Ok(record::Seen::Corrupt(_)) | Err(_) => {
            inner.doubt();
            return Err(Failed::Retry);
        }
    }
    let server_now = io.clock.now_server_ms(inner.now());
    inner.no_newer_claim(server_now).map_err(fenced)?;
    // A beat that started before the deadline but finished after it revives nothing: the
    // deadline is still measured from the previous success.
    inner.deadline().map_err(|l| Failed::Lost(l.reason))?;
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
