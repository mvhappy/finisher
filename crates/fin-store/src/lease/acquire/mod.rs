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
//!      the folder forever (inferred; see [`Contender`]). When the newest claim is our own
//!      and no lease of ours holds it (this process held that epoch and lost it, or the
//!      server-data lock is free, so no process of this server data runs a lease), it is
//!      judged the same way but reported as [`LeaseState::Lost`]: we hold nothing, and no
//!      one else is shown as the holder;
//!    - **unreadable or bad checksum** → held (spec §3.5). Never taken over automatically,
//!      only by a confirmed takeover;
//!    - **ours**: if one of our leases in this process still heartbeats it → held; if it is
//!      at an epoch this process held and that lease is lost or dropped → ours to re-take at
//!      once ([`registry`]);
//!    - **released** by its holder's clean close ([`LeaseRecord::released`]) → free: taken
//!      at once (after the checks of ours above), through a fresh claim like any acquisition;
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
//!    whose age is unknown, is another acquisition in progress → back off. A takeover does
//!    not back off for our own lost claim (above): it is no acquisition in progress.
//! 2. Claim `base + 1` only (base = the highest of the record's epoch, every claim, every
//!    segment name's epoch and the last epoch this process held here, so epochs never
//!    repeat even with every lease file gone). If it is taken, someone else is acquiring
//!    from the same view (perhaps with a claim our listing cannot see yet): back off without
//!    creating anything. Stepping on to `base + 2` could never lead to an acquisition, but
//!    that claim would fence the winner.
//! 3. Re-read `lease.json`: it must be exactly what was judged (still missing, the same
//!    record, the same damaged bytes); else delete our claim and back off.
//! 4. Write ours (epoch, heartbeat 0, stamped with server time). When it was missing: staged
//!    whole in a temp file, then published without replacing ([`publish`]: a hard link, or
//!    `MoveFileExW` on Windows, else `create_new`; the file server decides, so a `lease.json`
//!    hidden by Windows' not-found cache still wins, and where hard links work a crash never
//!    leaves a partial record). Else in place.
//!    If the write fails, put back what was there (a record we created is removed), delete
//!    our claim and back off. A claim name still pending deletion (Windows) is retried
//!    briefly ([`claim::claim_next`]).
//! 5. Read it back: it must be ours. Torn while no claim above ours exists → rewrite once.
//!    Anything else → delete our claim and back off.
//! 6. Delete claims below our epoch (spec §3.5), register, start the heartbeat.

use std::path::{Path, PathBuf};
use std::time::Instant;

use fin_core::CoreError;

mod publish;
mod take;

use super::clock::{ProbeSample, ServerClock};
use super::record::{self, LeaseRecord, Seen};
use super::refusal::refused;
use super::registry::{self, Mine};
use super::{Lease, LeaseState, Params, ServerLock, claim, server_lock, valid_server_id};

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
    /// The server-data lock, if this server uses one ([`ServerLock`]).
    server_lock: Option<ServerLock>,
    /// Taken by the observation that found our own record left by a stopped process.
    locked: Option<server_lock::Held>,
    /// The record to re-take is one this process wrote (registry).
    leftover: bool,
    /// This observation found `lease.json` missing beside our own newest claim, whose lease
    /// nothing of this server holds any more ([`OwnLost`]).
    own_lost: Option<OwnLost>,
    /// For the next [`Contender::take`]: our own lost claim at this epoch is no acquisition
    /// in progress (step 1 does not back off for it).
    own_claim: Option<u64>,
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

/// `lease.json` is missing beside our own newest claim at `epoch`, and no lease of this
/// server holds it: this process held that epoch and lost it (`here`), or the server-data lock
/// is free, so no process of this server data holds a lease (an earlier run's claim). Others
/// see the folder as held; this server reports its lease as lost.
#[derive(Debug, Clone, Copy)]
struct OwnLost {
    epoch: u64,
    here: bool,
}

impl OwnLost {
    /// What happened, for status and refusals.
    fn what(self) -> String {
        if self.here {
            format!(
                "this server's lease at epoch {} was lost (lease.json was deleted)",
                self.epoch
            )
        } else {
            format!(
                "lease.json is missing beside this server's claim at epoch {} from an earlier run (no process of this server data holds a lease)",
                self.epoch
            )
        }
    }

    /// [`OwnLost::what`] and what comes next.
    fn reason(self, p: &Params) -> String {
        if p.auto_takeover {
            format!(
                "{}; it is re-acquired once that claim is stale (~{} s)",
                self.what(),
                p.stale_after.as_secs()
            )
        } else {
            format!(
                "{}; on a sync folder `fin project takeover --confirm` re-takes it",
                self.what()
            )
        }
    }
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
    /// A record its holder released on a clean close ([`LeaseRecord::released`]): free to
    /// take, through a fresh claim above it.
    Released(LeaseRecord),
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
            server_lock: None,
            locked: None,
            leftover: false,
            own_lost: None,
            own_claim: None,
            #[cfg(any(test, feature = "test-hooks"))]
            after_write: None,
        }
    }

    /// Uses the server-data lock at `lock` (module docs of [`super::server_lock`]): every
    /// acquisition holds it, and our own record left by a stopped process is re-taken at once.
    pub fn with_server_lock(mut self, lock: ServerLock) -> Contender {
        self.server_lock = Some(lock);
        self
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
            Verdict::Leftover(r) => {
                self.leftover = true;
                self.take(Expected::Record(r))
            }
            Verdict::Released(r) => self.take(Expected::Record(r)),
            Verdict::Stale { expected, .. } if self.params.auto_takeover => self.take(expected),
            Verdict::Stale { holder, epoch, .. } => Err(LeaseState::Stale { holder, epoch }),
            Verdict::Held { .. } if let Some(own) = self.own_lost.take() => Err(LeaseState::Lost {
                reason: own.reason(&self.params),
            }),
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
        // Our own lost claim: the same refusal codes, but no "other server" to speak of.
        let own = self.own_lost.take();
        self.own_claim = own.map(|o| o.epoch);
        let own_text = |sync_text: &str| own.map(|o| format!("{}; {sync_text}", o.what()));
        let taken = match verdict {
            Verdict::Free => self.take(Expected::Missing),
            Verdict::Leftover(r) | Verdict::Released(r) => self.take(Expected::Record(r)),
            Verdict::Stale { expected, .. } if auto || confirm => self.take(expected),
            Verdict::Stale { holder, epoch, .. } => {
                return Err(refused(
                    "CONFIRM_REQUIRED",
                    &own_text("on a sync folder only a confirmed takeover re-takes it").unwrap_or_else(|| {
                        "the holder looks stopped, but on a sync folder only a confirmed takeover replaces it".into()
                    }),
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
                        &own_text("on a sync folder re-taking it needs confirmation").unwrap_or_else(|| {
                            "another server holds the lease; on a sync folder taking it over needs confirmation".into()
                        }),
                        Some((&holder, epoch, age)),
                    ));
                }
                _ => {
                    return Err(refused(
                        why,
                        &own.map(|o| o.reason(&self.params))
                            .unwrap_or_else(|| "another server holds the lease".into()),
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
        self.own_lost = None;
        self.own_claim = None;
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
                if holder == self.server_id {
                    self.own_lost = self.own_claim_lost(newest.epoch);
                }
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
                        Mine::Unknown => {
                            if let Some(v) = self.own_record_elsewhere(&r) {
                                return Ok(v);
                            }
                        }
                    }
                }
                if r.released {
                    self.evidence = None;
                    return Ok(Verdict::Released(r));
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

    /// Our own newest claim at `epoch` beside a missing `lease.json`: whether no lease of this
    /// server holds it ([`OwnLost`]). To others the folder is held (by us); this server knows
    /// better when this process held that epoch and lost it, or when the server-data lock is
    /// free (taken only to ask, and released at once). A lease of ours still running here, or
    /// a lock another process holds, keeps it held.
    fn own_claim_lost(&self, epoch: u64) -> Option<OwnLost> {
        let key = registry::key(&self.dir, &self.server_id);
        if registry::stopped_at(&key, epoch) {
            return Some(OwnLost { epoch, here: true });
        }
        if registry::live(&key) {
            return None;
        }
        // As for the restart shortcut: a free OS lock proves nothing when the server data is
        // on network or sync storage.
        let cfg = self.server_lock.as_ref().filter(|c| c.shortcut)?;
        match server_lock::try_lock(&cfg.path) {
            server_lock::Attempt::Locked(held) => {
                drop(held);
                Some(OwnLost { epoch, here: false })
            }
            _ => None,
        }
    }

    /// Our own record, not written by this process: the server-data lock decides
    /// ([`super::server_lock`]). `None`: judge it like anyone's.
    fn own_record_elsewhere(&mut self, r: &LeaseRecord) -> Option<Verdict> {
        let cfg = self.server_lock.as_ref()?;
        match server_lock::try_lock(&cfg.path) {
            server_lock::Attempt::Locked(held)
                if cfg.shortcut && server_lock::same_host(&r.holder) =>
            {
                self.evidence = None;
                self.locked = Some(held);
                Some(Verdict::Leftover(r.clone()))
            }
            server_lock::Attempt::Busy => {
                self.evidence = None;
                Some(Verdict::Held {
                    holder: r.holder.clone(),
                    epoch: r.epoch,
                    age: None,
                    expected: None,
                    why: "HELD",
                })
            }
            _ => None,
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
}
