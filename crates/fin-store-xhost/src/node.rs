//! `holder`, `contender` and `follower`: one store driven like `fin-node`'s maintenance
//! thread (`tick_with` without the heavy steps), with commits and stalls.

use std::sync::Arc;
use std::time::{Duration, Instant};

use fin_core::{CoreError, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::lease::{HangPoint, LeaseState, MonoClock, SystemClock};
use fin_store::{MAIN, OpenOptions, ReadOnlyReason, Store, StoreEvent, TickOptions, TxStatus};
use serde_json::{Value, json};

use crate::db::Db;
use crate::emit::{Emitter, lease_parts};
use crate::{Args, StallMode};

const LIGHT: TickOptions = TickOptions {
    backup: false,
    compaction: false,
};

/// The system clock, with the share's time seen `shift_ms` off (`--skew-ms`): every probe
/// mtime this store reads is shifted, as a client-set mtime on a skewed client would be.
#[derive(Debug)]
struct SkewClock {
    shift_ms: i64,
}

impl MonoClock for SkewClock {
    fn now(&self) -> Instant {
        SystemClock.now()
    }
    fn server_shift_ms(&self) -> i64 {
        self.shift_ms
    }
}

/// What was last reported, to emit changes only.
#[derive(Default)]
struct Seen {
    lease: Option<LeaseState>,
    read_only: Option<Option<ReadOnlyReason>>,
    beats: Option<(u64, u64, u64)>,
    export_error: Option<String>,
    fork: Option<String>,
    /// When the last `alive` sample was emitted.
    alive: Option<Instant>,
}

/// The lease state without its age (which changes every look) or the holder's wording (a
/// backed-off acquisition names the claim's server id, a judged record its description).
fn same_lease(a: &LeaseState, b: &LeaseState) -> bool {
    match (a, b) {
        (LeaseState::HeldElsewhere { epoch, .. }, LeaseState::HeldElsewhere { epoch: e2, .. }) => {
            epoch == e2
        }
        _ => a == b,
    }
}

struct Node<'a> {
    a: &'a Args,
    em: &'a mut Emitter,
    store: Store,
    db: Db,
    seen: Seen,
    role: &'static str,
    commits: u64,
    ever_held: bool,
    accepted: Vec<String>,
}

pub fn run(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let role = match a.cmd.as_str() {
        "follower" => "follower",
        "contender" => "contender",
        _ => "holder",
    };
    let mut opts = OpenOptions::default();
    if a.skew_ms != 0 {
        opts.clock = Some(Arc::new(SkewClock {
            shift_ms: a.skew_ms,
        }) as Arc<dyn MonoClock>);
    }
    let t0 = Instant::now();
    let store = Store::open_with(&a.data, &a.dir, opts).map_err(|e| {
        let e: CoreError = e.into();
        format!("open: {}: {}", e.code, e.message)
    })?;
    em.server = Some(store.server_id().to_string());
    let db = Db::open(store.db_path())?;
    let mut n = Node {
        a,
        em,
        store,
        db,
        seen: Seen::default(),
        role,
        commits: 0,
        ever_held: false,
        accepted: Vec::new(),
    };
    let lease = n.store.lease_state();
    n.em.emit(
        "opened",
        Some(&lease),
        json!({
            "role": role, "projectId": n.store.project_id(), "storage": n.store.storage_kind(),
            "openMs": t0.elapsed().as_millis() as u64, "head": n.store.head(MAIN),
            "readOnly": n.store.read_only(), "leaseDetail": lease,
        }),
    );
    if role == "follower" && !a.active {
        if matches!(lease, LeaseState::Held { .. }) {
            n.em.emit(
                "warning",
                Some(&lease),
                json!({ "what": "followerAcquiredAtOpen" }),
            );
        }
        // Passive: imports only, never acquires (a stand-in for a lease held elsewhere).
        n.store.set_test_lease(0, false);
    }
    n.observe("open");
    n.drain_open_events()?;
    n.lp()?;
    n.finish()
}

impl Node<'_> {
    fn lease(&self) -> LeaseState {
        self.store.lease_state()
    }

    /// Emits what changed in memory: the lease, the read-only reason, heartbeats. No I/O.
    fn observe(&mut self, why: &str) {
        let l = self.lease();
        if self.seen.lease.as_ref().is_none_or(|p| !same_lease(p, &l)) {
            let from = self.seen.lease.as_ref().map(|p| lease_parts(p).0);
            let gained = matches!(l, LeaseState::Held { .. });
            self.ever_held |= gained;
            let previous = self.store.lease().filter(|_| gained).map(|x| {
                let p = x.previous();
                json!({ "record": p.record, "damaged": p.damaged, "claims": p.claims })
            });
            let reason = match &l {
                LeaseState::Lost { reason } => json!(reason),
                _ => Value::Null,
            };
            self.em.emit(
                "lease",
                Some(&l),
                json!({ "from": from, "detail": l, "reason": reason, "previous": previous, "at": why }),
            );
            self.seen.lease = Some(l.clone());
        }
        // A sample at least every 500 ms: the analyser bounds what this process believed
        // between changes by these.
        if self
            .seen
            .alive
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(500))
        {
            self.em.emit("alive", Some(&l), json!({ "at": why }));
            self.seen.alive = Some(Instant::now());
        }
        let ro = self.store.read_only();
        if self.seen.read_only != Some(ro) {
            self.em.emit("readOnly", Some(&l), json!({ "reason": ro }));
            self.seen.read_only = Some(ro);
        }
        if let Some(x) = self.store.lease() {
            let b = (x.epoch(), x.heartbeats(), x.failed_heartbeats());
            if self.seen.beats != Some(b) {
                let ok = self.seen.beats.is_none_or(|p| p.0 != b.0 || p.1 != b.1);
                self.em.emit(
                    if ok { "beat" } else { "beatFailed" },
                    Some(&l),
                    json!({ "beatEpoch": b.0, "heartbeats": b.1, "failed": b.2 }),
                );
                self.seen.beats = Some(b);
            }
        }
    }

    fn drain_open_events(&mut self) -> Result<(), String> {
        let r = self
            .store
            .tick_with(Instant::now(), LIGHT)
            .map_err(|e| e.to_string())?;
        self.events(r.events);
        Ok(())
    }

    fn events(&mut self, events: Vec<StoreEvent>) {
        for e in events {
            match e {
                StoreEvent::Applied(c) => {
                    let row = self.db.row(&c.tx.tx_id);
                    let l = self.lease();
                    self.em.emit(
                        "applied",
                        Some(&l),
                        json!({
                            "txId": c.tx.tx_id, "seq": c.rev, "origin": c.tx.author,
                            "txHash": row.as_ref().map(|r| r.tx_hash.clone()),
                            "recEpoch": row.as_ref().map(|r| r.epoch),
                        }),
                    );
                }
                StoreEvent::Rewound {
                    branch,
                    from_rev,
                    quarantined,
                    own,
                    generation,
                } => {
                    let l = self.lease();
                    self.em.emit(
                        "rewound",
                        Some(&l),
                        json!({ "branch": branch, "fromRev": from_rev, "quarantined": quarantined, "own": own, "generation": generation }),
                    );
                }
                StoreEvent::Rebuilt { incarnation } => {
                    self.em
                        .emit("rebuilt", None, json!({ "incarnation": incarnation }));
                }
                StoreEvent::Status(_) => {}
            }
        }
    }

    fn tick(&mut self) -> Result<(), String> {
        let t = Instant::now();
        let r = self.store.tick_with(t, LIGHT).map_err(|e| e.to_string())?;
        let ms = t.elapsed().as_millis() as u64;
        let l = self.lease();
        if ms >= 1000 {
            self.em.emit("slowTick", Some(&l), json!({ "ms": ms }));
        }
        if r.export.segments_written > 0 {
            self.em.emit(
                "export",
                Some(&l),
                json!({ "segments": r.export.segments_written, "records": r.export.records }),
            );
        }
        let err = r
            .export
            .error
            .as_ref()
            .map(|e| format!("{}: {}", e.code, e.message));
        if err != self.seen.export_error {
            self.em
                .emit("exportError", Some(&l), json!({ "error": err }));
            self.seen.export_error = err;
        }
        for i in &r.imports {
            let fork = i.fork.as_ref().map(|f| format!("{f:?}"));
            if fork != self.seen.fork {
                self.em.emit("fork", Some(&l), json!({ "fork": fork }));
                self.seen.fork = fork;
            }
            if let Some(e) = &i.error {
                self.em.emit(
                    "importError",
                    Some(&l),
                    json!({ "code": e.code, "message": e.message }),
                );
            }
        }
        self.events(r.events);
        self.observe("tick");
        Ok(())
    }

    fn commit(&mut self) {
        self.commits += 1;
        let tx_id = format!("{}.{}.{}", self.a.label, std::process::id(), self.commits);
        let author = format!("xhost:{}:{}", self.a.label, self.store.server_id());
        let tx = Tx {
            tx_id: tx_id.clone(),
            author,
            branch: MAIN.into(),
            base_rev: self.store.head(MAIN),
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            ops: vec![Op::AssetAdd {
                id: "$a".into(),
                path: format!("/xhost/{tx_id}.mov"),
                duration: Rational::from_int(10),
                video: Some(VideoInfo {
                    width: 1920,
                    height: 1080,
                    rate: Rational::from_int(30),
                }),
                audio: None,
                fingerprint: format!("fp-{tx_id}"),
            }],
        };
        let before = self.lease();
        let t = Instant::now();
        let r = self.store.commit(tx);
        let ms = t.elapsed().as_millis() as u64;
        let l = self.lease();
        match r {
            Ok(c) => {
                let row = self.db.row(&tx_id);
                self.accepted.push(tx_id.clone());
                self.em.emit(
                    "commit",
                    Some(&l),
                    json!({
                        "ok": true, "txId": tx_id, "seq": c.rev,
                        "txHash": row.as_ref().map(|r| r.tx_hash.clone()),
                        "rowEpoch": row.as_ref().map(|r| r.epoch),
                        "leaseBefore": lease_parts(&before).1, "ms": ms,
                        "ids": c.bindings.values().collect::<Vec<_>>(),
                    }),
                );
            }
            Err(e) => {
                let e: CoreError = e.into();
                let reason = e.details.get("reason").cloned();
                self.em.emit(
                    "commit",
                    Some(&l),
                    json!({ "ok": false, "txId": tx_id, "code": e.code, "reason": reason, "message": e.message, "ms": ms }),
                );
            }
        }
        self.observe("commit");
    }

    /// The main loop: ticks, commits (holder, contender), stalls, clock samples.
    fn lp(&mut self) -> Result<(), String> {
        let a = self.a;
        let start = Instant::now();
        let end = start + Duration::from_secs_f64(a.secs);
        let tick_every = Duration::from_millis(a.tick_ms);
        let commit_every = Duration::from_millis(a.commit_every_ms);
        let clock_every = Duration::from_millis(a.clock_every_ms);
        let stall = a
            .stall_at_ms
            .filter(|_| self.role != "follower" && a.stall_for_ms > 0)
            .map(|at| {
                let s = start + Duration::from_millis(at);
                (s, s + Duration::from_millis(a.stall_for_ms))
            });
        let mut stalled = false;
        let mut beats_failing_until: Option<Instant> = None;
        let (mut next_tick, mut next_commit, mut next_clock) = (start, start, start);
        while Instant::now() < end {
            let now = Instant::now();
            if let Some((s_at, s_end)) = stall
                && !stalled
                && now >= s_at
            {
                stalled = true;
                if let Some(until) = self.stall(s_end)? {
                    beats_failing_until = Some(until);
                }
                continue;
            }
            if let Some(u) = beats_failing_until
                && Instant::now() >= u
            {
                if let Some(l) = self.store.lease() {
                    l.fail_heartbeats(0);
                }
                beats_failing_until = None;
                let l = self.lease();
                self.em.emit("stall", Some(&l), json!({ "phase": "end" }));
            }
            if now >= next_clock {
                let l = self.lease();
                self.em.clock_sample(&a.dir, Some(&l));
                next_clock = now + clock_every;
            }
            if now >= next_tick {
                self.tick()?;
                next_tick = Instant::now() + tick_every;
            }
            if self.role != "follower" && Instant::now() >= next_commit {
                // Attempts before the first acquisition are logged too (refusals are
                // evidence), at most one per second.
                self.commit();
                let gap = if self.ever_held {
                    commit_every
                } else {
                    commit_every.max(Duration::from_secs(1))
                };
                next_commit = Instant::now() + gap;
            }
            let wake = next_tick.min(next_commit).min(next_clock).min(end);
            let now = Instant::now();
            if wake > now {
                std::thread::sleep((wake - now).min(Duration::from_millis(50)));
            }
        }
        Ok(())
    }

    /// Starts the stall; returns when the heartbeats must be restored (modes that go on
    /// ticking), after blocking through the stall for modes that stop ticking.
    fn stall(&mut self, s_end: Instant) -> Result<Option<Instant>, String> {
        let mode = self.a.stall_mode;
        if mode == StallMode::Freeze && self.role != "follower" {
            // A write in flight: committed, not exported yet.
            self.commit();
        }
        let fail_beats = matches!(
            mode,
            StallMode::Freeze | StallMode::Beats | StallMode::Check
        );
        if fail_beats && let Some(l) = self.store.lease() {
            l.fail_heartbeats(u32::MAX);
        }
        if mode == StallMode::Check
            && let Some(l) = self.store.lease()
        {
            let hooks = l.hooks();
            hooks.hang(HangPoint::CheckAfterVerify);
            let wait = s_end.saturating_duration_since(Instant::now());
            std::thread::spawn(move || {
                std::thread::sleep(wait);
                hooks.release();
            });
        }
        let l = self.lease();
        let mode_s = format!("{mode:?}").to_lowercase();
        self.em.emit(
            "stall",
            Some(&l),
            json!({ "phase": "start", "mode": mode_s }),
        );
        if matches!(mode, StallMode::Beats | StallMode::Check) {
            return Ok(Some(s_end));
        }
        // Freeze, ticks: no folder work, no commits; the in-memory state is still sampled
        // (it does no I/O), so a heartbeat or a self-fence during the stall is seen.
        while Instant::now() < s_end {
            std::thread::sleep(Duration::from_millis(100));
            self.observe("stall");
        }
        if fail_beats && let Some(l) = self.store.lease() {
            l.fail_heartbeats(0);
        }
        let l = self.lease();
        self.em
            .emit("stall", Some(&l), json!({ "phase": "end", "mode": mode_s }));
        Ok(None)
    }

    /// Final report: what became of every accepted commit, then close, then the chain.
    fn finish(self) -> Result<(), String> {
        let mut statuses = serde_json::Map::new();
        for id in &self.accepted {
            let s = match self.store.tx_status(id) {
                Ok(TxStatus::Committed(c)) => json!({ "state": "committed", "seq": c.rev }),
                Ok(TxStatus::SetAside { reason, origin, .. }) => {
                    json!({ "state": "setAside", "reason": reason, "origin": origin })
                }
                Ok(TxStatus::Unknown) => json!({ "state": "unknown" }),
                Err(e) => json!({ "state": "error", "error": e.to_string() }),
            };
            statuses.insert(id.clone(), s);
        }
        let status = self.store.status();
        let l = self.lease();
        self.em.emit(
            "final",
            Some(&l),
            json!({
                "role": self.role, "attempts": self.commits, "accepted": self.accepted.len(),
                "txStatus": statuses, "head": self.store.head(MAIN),
                "generation": self.store.generation(MAIN), "readOnly": self.store.read_only(),
                "quarantine": status.quarantine, "warnings": status.warnings,
            }),
        );
        let Node { em, store, db, .. } = self;
        match store.close() {
            Ok(r) => em.emit(
                "closed",
                Some(&l),
                json!({ "segments": r.segments_written, "records": r.records, "error": r.error }),
            ),
            Err(e) => em.emit("closed", Some(&l), json!({ "error": e.to_string() })),
        }
        em.emit(
            "chain",
            None,
            json!({ "rows": db.chain()?, "quarantine": db.quarantine()? }),
        );
        Ok(())
    }
}
