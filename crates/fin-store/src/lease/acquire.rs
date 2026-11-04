//! Acquiring and taking over (spec §3.5): [`Contender`], a server that wants the lease and
//! remembers what it observed between attempts.
//!
//! # One observation ([`Contender::observe`])
//!
//! 1. Probe server time ([`ServerClock::probe`]). The round counts towards staleness only if
//!    the probe moved forward and its round trip is ≤ TTL/4 (spec §3.5 table).
//! 2. Read `lease.json` uncached:
//!    - **missing**, no claims → free;
//!    - **missing** beside claims → held (spec §3.5). Its age is the newest claim's mtime in
//!      server time: an acquirer that crashed between claim and record would otherwise hold
//!      the folder forever (inferred; see [`Contender`]);
//!    - **unreadable or bad checksum** → held (spec §3.5). Never taken over automatically,
//!      only by a confirmed takeover;
//!    - **ours**: if one of our leases in this process still heartbeats it → held; if it is
//!      at an epoch this process held and that lease is lost or dropped → ours to re-take at
//!      once ([`registry`]);
//!    - otherwise **held**, with age = this round's server time − the record's stamp.
//! 3. Staleness, all of:
//!    - age > 45 s in two counting rounds ≥ 5 s apart (monotonic), with no non-stale counting
//!      round between them;
//!    - the identical record (or claim set) seen for ≥ [`Params::min_unchanged`] (25 s) by
//!      our own monotonic clock. A running holder rewrites it within 20 s or fences itself,
//!      so it is never taken over, however wrong either clock is;
//!    - observations no more than 2 × 45 s apart (older evidence is discarded).
//!
//!    Any change of the record starts over; a round that does not count changes nothing.
//!
//! # Taking ([`Contender::take`])
//!
//! 1. List claims. A claim above the expected record's epoch that is younger than 45 s
//!    (server time from the last counting probe, extrapolated so it never overestimates), or
//!    whose age is unknown, is another acquisition in progress → back off.
//! 2. Claim `base+1..base+4` (base = the highest of the record's epoch and every claim).
//!    Needing more than the first name means a claim invisible to our listing exists, i.e. a
//!    fresh one (Windows hides new files for up to 9.9 s) → delete ours and back off. So of
//!    two racing contenders, the lower claim proceeds and the higher backs off.
//! 3. Re-read `lease.json`: it must be exactly what was judged (still missing, the same
//!    record, the same damaged bytes); else delete our claim and back off.
//! 4. Write ours (epoch, heartbeat 0, stamped with server time): `create_new` when it was
//!    missing (the file server decides, so a `lease.json` hidden by Windows' not-found cache
//!    still wins), else in place. If the write fails, put back what was there (a record we
//!    created is removed), delete our claim and back off.
//! 5. Read it back: it must be ours. Torn while no claim above ours exists → rewrite once.
//!    Anything else → delete our claim and back off.
//! 6. Delete claims below our epoch (spec §3.5), register, start the heartbeat.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use fin_core::CoreError;

use super::clock::{ProbeSample, SAMPLE_MAX_AGE, ServerClock};
use super::record::{self, LeaseRecord, Seen};
use super::refusal::refused;
use super::registry::{self, Mine};
use super::{Lease, LeaseState, Params, claim, valid_server_id};
use crate::fsio;

/// A server that wants the lease of one folder. Keep one across attempts: staleness needs two
/// observations ≥ 5 s apart.
///
/// Inferred (the spike is not available; these follow the spec's rules):
/// - the back-off rules of [`Contender::take`] steps 1–2;
/// - a missing `lease.json` beside claims ages like a record, by the newest claim's mtime;
/// - re-taking our own record at once when this process held its epoch and that lease is
///   lost or dropped;
/// - restoring the previous record after a failed write, and rewriting a torn read-back;
/// - on network and local storage a takeover never displaces a live holder, confirmed or not
///   (`confirm` matters on sync folders, and for a damaged `lease.json`).
pub struct Contender {
    dir: PathBuf,
    server_id: String,
    holder: String,
    params: Params,
    clock: ServerClock,
    evidence: Option<Evidence>,
    /// The last probe that counted (moved forward, round trip ≤ TTL/4); used to age rival
    /// claims.
    counted: Option<ProbeSample>,
    #[cfg(any(test, feature = "test-hooks"))]
    after_write: Option<AfterWrite>,
}

/// Test hook run after the record is written ([`Contender::after_write_for_test`]).
#[cfg(any(test, feature = "test-hooks"))]
type AfterWrite = Box<dyn FnOnce(&Path) + Send + Sync>;

/// What is now in the folder, since when (monotonic) it has been seen unchanged, and since
/// when it has been seen stale.
#[derive(Debug)]
struct Evidence {
    seen: Fingerprint,
    first_seen: Instant,
    stale_since: Option<Instant>,
    last_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Fingerprint {
    Record(LeaseRecord),
    Claims(Vec<u64>),
}

/// What [`Contender::take`] expects to replace.
#[derive(Debug, Clone)]
enum Expected {
    Missing,
    Record(LeaseRecord),
    Corrupt(Vec<u8>),
}

enum Verdict {
    Free,
    Leftover(LeaseRecord),
    Stale {
        holder: String,
        epoch: u64,
        expected: Expected,
    },
    Held {
        holder: String,
        epoch: u64,
        age: Option<i64>,
        /// What a confirmed takeover would replace (`None`: never, e.g. our own live lease).
        expected: Option<Expected>,
        /// `HELD` or `STALE_UNCONFIRMED` (refusal reason).
        why: &'static str,
    },
}

impl std::fmt::Debug for Contender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contender")
            .field("dir", &self.dir)
            .field("server_id", &self.server_id)
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

impl Contender {
    pub fn new(dir: &Path, server_id: &str, holder: &str, params: Params) -> Contender {
        Contender {
            dir: dir.to_path_buf(),
            server_id: server_id.to_string(),
            holder: holder.to_string(),
            clock: ServerClock::new(dir, server_id, params.clock.clone()),
            params,
            evidence: None,
            counted: None,
            #[cfg(any(test, feature = "test-hooks"))]
            after_write: None,
        }
    }

    /// One attempt: takes a free lease, our own stopped one, or (where
    /// [`Params::auto_takeover`]) a confirmed-stale one. Otherwise why not: `HeldElsewhere`,
    /// `Stale` (sync folders: ask the user), or `Lost` when the folder cannot be reached.
    /// After success, import before writing (module docs of [`super`]).
    pub fn try_acquire(&mut self) -> Result<Lease, LeaseState> {
        match self
            .observe()
            .map_err(|reason| LeaseState::Lost { reason })?
        {
            Verdict::Free => self.take(Expected::Missing),
            Verdict::Leftover(r) => self.take(Expected::Record(r)),
            Verdict::Stale { expected, .. } if self.params.auto_takeover => self.take(expected),
            Verdict::Stale { holder, epoch, .. } => Err(LeaseState::Stale { holder, epoch }),
            Verdict::Held {
                holder, epoch, age, ..
            } => Err(LeaseState::HeldElsewhere {
                holder,
                epoch,
                age_server_ms: age,
            }),
        }
    }

    /// `fin project takeover [--confirm]`: like [`Contender::try_acquire`], and also
    /// - with `confirm` on a sync folder: takes the lease whatever its state (the lease is
    ///   advisory there; the user has said the other machine stopped);
    /// - with `confirm` anywhere: replaces a damaged `lease.json`.
    ///
    /// Otherwise `LEASE_TAKEOVER_REFUSED` with `details.reason`: `CONFIRM_REQUIRED` (sync),
    /// `HELD` (a live holder), `STALE_UNCONFIRMED` (looks stopped; the confirming probe comes
    /// ≥ 5 s after the first), `CONTENDED` (another server is acquiring) or `UNAVAILABLE`
    /// (the folder cannot be reached). After success, import before writing.
    pub fn takeover(&mut self, confirm: bool) -> Result<Lease, CoreError> {
        let auto = self.params.auto_takeover;
        let verdict = self
            .observe()
            .map_err(|r| refused("UNAVAILABLE", &r, None))?;
        let taken = match verdict {
            Verdict::Free => self.take(Expected::Missing),
            Verdict::Leftover(r) => self.take(Expected::Record(r)),
            Verdict::Stale { expected, .. } if auto || confirm => self.take(expected),
            Verdict::Stale { holder, epoch, .. } => {
                return Err(refused(
                    "CONFIRM_REQUIRED",
                    "the holder looks stopped, but on a sync folder only a confirmed takeover replaces it",
                    Some((&holder, epoch, None)),
                ));
            }
            Verdict::Held {
                holder,
                epoch,
                age,
                expected,
                why,
            } => match expected {
                Some(e) if confirm && (!auto || matches!(e, Expected::Corrupt(_))) => self.take(e),
                Some(_) if !auto => {
                    return Err(refused(
                        "CONFIRM_REQUIRED",
                        "another server holds the lease; on a sync folder taking it over needs confirmation",
                        Some((&holder, epoch, age)),
                    ));
                }
                _ => {
                    return Err(refused(
                        why,
                        "another server holds the lease",
                        Some((&holder, epoch, age)),
                    ));
                }
            },
        };
        taken.map_err(|s| match s {
            LeaseState::Lost { reason } => refused("UNAVAILABLE", &reason, None),
            LeaseState::HeldElsewhere {
                holder,
                epoch,
                age_server_ms,
            } => refused(
                "CONTENDED",
                "another server acquired or is acquiring the lease",
                Some((&holder, epoch, age_server_ms)),
            ),
            other => refused("CONTENDED", &format!("{other:?}"), None),
        })
    }

    /// Test hook: `f(dir)` runs once, right after the next acquisition writes its record and
    /// before it reads it back (another writer, or a torn write).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn after_write_for_test(&mut self, f: impl FnOnce(&Path) + Send + Sync + 'static) {
        self.after_write = Some(Box::new(f));
    }

    fn record_path(&self) -> PathBuf {
        self.dir.join(record::FILE)
    }

    /// One observation (module docs). `Err`: the folder cannot be reached.
    fn observe(&mut self) -> Result<Verdict, String> {
        if !valid_server_id(&self.server_id) {
            return Err(format!("{:?} is not a valid server id", self.server_id));
        }
        let probe = self
            .clock
            .probe()
            .map_err(|e| format!("cannot write the probe file in {}: {e}", self.dir.display()))?;
        let counts = probe.advanced && probe.sample.rtt <= self.params.max_rtt;
        let server_now = counts.then_some(probe.sample.server_ms);
        if counts {
            self.counted = Some(probe.sample);
        }
        let at = probe.sample.at;
        let unknown = |holder: String, expected| Verdict::Held {
            holder,
            epoch: 0,
            age: None,
            expected,
            why: "HELD",
        };
        let seen = match record::read(&self.record_path()) {
            Ok(s) => s,
            Err(e) => {
                self.evidence = None;
                return Ok(unknown(
                    format!("unknown (lease.json is unreadable: {e})"),
                    None,
                ));
            }
        };
        match seen {
            Seen::Missing => {
                let claims = claim::list(&self.dir)
                    .map_err(|e| format!("cannot list {}: {e}", self.dir.display()))?;
                let Some(newest) = claims.last() else {
                    self.evidence = None;
                    return Ok(Verdict::Free);
                };
                let holder =
                    claim::owner(&self.dir, newest.epoch).unwrap_or_else(|| "unknown".to_string());
                let age = server_now.zip(newest.server_ms).map(|(n, m)| n - m);
                let seen = Fingerprint::Claims(claims.iter().map(|c| c.epoch).collect());
                Ok(self.judge(
                    seen,
                    at,
                    counts,
                    age,
                    holder,
                    newest.epoch,
                    Expected::Missing,
                ))
            }
            Seen::Corrupt(bytes) => {
                self.evidence = None;
                Ok(unknown(
                    "unknown (lease.json is damaged)".to_string(),
                    Some(Expected::Corrupt(bytes)),
                ))
            }
            Seen::Valid(r) => {
                if r.server_id == self.server_id {
                    match registry::lookup(&registry::key(&self.dir, &self.server_id), &r) {
                        Mine::Live => {
                            self.evidence = None;
                            return Ok(Verdict::Held {
                                holder: r.holder.clone(),
                                epoch: r.epoch,
                                age: None,
                                expected: None,
                                why: "HELD",
                            });
                        }
                        Mine::Leftover => {
                            self.evidence = None;
                            return Ok(Verdict::Leftover(r));
                        }
                        Mine::Unknown => {}
                    }
                }
                let age = server_now.map(|n| n - r.probe_server_ms);
                let (holder, epoch) = (r.holder.clone(), r.epoch);
                Ok(self.judge(
                    Fingerprint::Record(r.clone()),
                    at,
                    counts,
                    age,
                    holder,
                    epoch,
                    Expected::Record(r),
                ))
            }
        }
    }

    /// Step 3 of an observation.
    #[allow(clippy::too_many_arguments)]
    fn judge(
        &mut self,
        seen: Fingerprint,
        at: Instant,
        counts: bool,
        age: Option<i64>,
        holder: String,
        epoch: u64,
        expected: Expected,
    ) -> Verdict {
        let p = &self.params;
        // Evidence is about what is in the folder now and recent: a change, or a gap of more
        // than 2 × stale-after since the last look, starts it over.
        let ev = match self.evidence.take() {
            Some(e)
                if e.seen == seen
                    && at.saturating_duration_since(e.last_at) <= p.stale_after * 2 =>
            {
                Evidence { last_at: at, ..e }
            }
            _ => Evidence {
                seen,
                first_seen: at,
                stale_since: None,
                last_at: at,
            },
        };
        let ev = self.evidence.insert(ev);
        let held = |why| Verdict::Held {
            holder: holder.clone(),
            epoch,
            age,
            expected: Some(expected.clone()),
            why,
        };
        if !counts {
            // Undecided this round: keep what was seen before.
            return held(if ev.stale_since.is_some() {
                "STALE_UNCONFIRMED"
            } else {
                "HELD"
            });
        }
        let stale_ms = i64::try_from(p.stale_after.as_millis()).unwrap_or(i64::MAX);
        if !age.is_some_and(|a| a > stale_ms) {
            ev.stale_since = None;
            return held("HELD");
        }
        let since = *ev.stale_since.get_or_insert(at);
        if at.saturating_duration_since(since) >= p.confirm_gap
            && at.saturating_duration_since(ev.first_seen) >= p.min_unchanged
        {
            Verdict::Stale {
                holder,
                epoch,
                expected,
            }
        } else {
            held("STALE_UNCONFIRMED")
        }
    }

    /// Claims a fresh epoch and writes our record over `expected` (module docs).
    fn take(&mut self, expected: Expected) -> Result<Lease, LeaseState> {
        self.evidence = None;
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
        let server_now = self.counted.and_then(|p| counted_server_ms(&p, mono.now()));
        let stale_ms = i64::try_from(self.params.stale_after.as_millis()).unwrap_or(i64::MAX);
        let in_progress = |c: &claim::ClaimFile| match (server_now, c.server_ms) {
            (Some(now), Some(m)) => now - m <= stale_ms,
            _ => true,
        };
        if let Some(rival) = claims
            .iter()
            .rev()
            .find(|c| c.epoch > floor && in_progress(c))
        {
            let who = claim::owner(&self.dir, rival.epoch)
                .unwrap_or_else(|| "another server (acquiring)".to_string());
            return Err(busy(who, rival.epoch));
        }
        // 2. Claim.
        let base = claims.last().map_or(floor, |c| c.epoch.max(floor));
        let claimed = match claim::claim_next(&self.dir, base, &self.server_id) {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                return Err(busy("another server (acquiring)".into(), base + 1));
            }
            Err(e) => return Err(lost(format!("cannot create a lease claim: {e}"))),
        };
        let epoch = claimed.epoch;
        let back_off = |state: LeaseState| {
            claim::remove(&self.dir, epoch);
            Err(state)
        };
        if claimed.contended {
            return back_off(busy("another server (acquiring)".into(), epoch - 1));
        }
        let stamped = mono.now();
        let Some(stamp) = self.clock.now_server_ms(stamped) else {
            return back_off(lost("no server time: the probe file cannot be read".into()));
        };
        let mine = LeaseRecord::new(&self.server_id, epoch, 0, &self.holder, stamp);
        let bytes = record::encode(&mine);
        let path = self.record_path();
        // 3. Unchanged since judged? 4. Then write.
        let wrote = match (&expected, record::read(&path)) {
            (Expected::Missing, Ok(Seen::Missing)) => create_record(&path, &bytes),
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
                // A record we created is already removed by `create_record`.
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
        // 6.
        claim::remove_below(&self.dir, epoch);
        Ok(Lease::start(
            &self.dir,
            registry::key(&self.dir, &self.server_id),
            self.params.clone(),
            self.clock.clone(),
            mine,
            stamped,
        ))
    }
}

/// Server time at `now` from a counting probe, extrapolated from the end of its round trip,
/// so it never overestimates (a rival claim never looks older than it is); `None` once the
/// sample is older than [`SAMPLE_MAX_AGE`].
fn counted_server_ms(p: &ProbeSample, now: Instant) -> Option<i64> {
    let since = now.checked_duration_since(p.at)?;
    if since > SAMPLE_MAX_AGE {
        return None;
    }
    let after = since.saturating_sub(p.rtt);
    Some(p.server_ms + i64::try_from(after.as_millis()).unwrap_or(i64::MAX))
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

/// Creates `lease.json` exclusively, the only time it is ever created. If the write fails the
/// file is removed again: a partial record would read as damaged to everyone, and a damaged
/// record is never taken over automatically. It is ours and was never valid, so nobody holds
/// it.
fn create_record(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = file.write_all(bytes).and_then(|()| fsio::sync_file(&file));
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    result
}
