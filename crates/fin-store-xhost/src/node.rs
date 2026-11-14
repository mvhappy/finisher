//! `holder`, `contender` and `follower`: one store driven like `fin-node`'s maintenance
//! thread (`tick_with` without the heavy steps), with commits and stalls. [`Node`] is also
//! the base of `brancher` and `merger` (`history`).

use std::time::{Duration, Instant};

use fin_core::{CommittedTx, CoreError, Op, SCHEMA_VERSION, Tx};
use fin_store::lease::{HangPoint, LeaseState};
use fin_store::{MAIN, ReadOnlyReason, Store, StoreEvent, TickOptions, TxStatus};
use serde_json::{Value, json};

use crate::db::Db;
use crate::emit::{Emitter, lease_parts};
use crate::hashes;
use crate::ops::{asset_op, kind_of, open_store};
use crate::{Args, StallMode};

const LIGHT: TickOptions = TickOptions {
    backup: false,
    compaction: false,
};

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

pub struct Node<'a> {
    pub a: &'a Args,
    pub em: &'a mut Emitter,
    pub store: Store,
    pub db: Db,
    seen: Seen,
    pub role: &'static str,
    commits: u64,
    ever_held: bool,
    /// Accepted records' txIds (commits, branch records, merge records).
    pub accepted: Vec<String>,
    /// Records applied by imports so far.
    pub applied: u64,
    /// The txId of the first applied record of kind `--until-kind`, when seen.
    pub until_seen: Option<String>,
}

pub fn run(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let role = match a.cmd.as_str() {
        "follower" => "follower",
        "contender" => "contender",
        _ => "holder",
    };
    let mut n = open(a, em, role)?;
    n.lp()?;
    n.finish(true)
}

/// Opens the store of `a` as `role`, reports it, and drains the events of the open.
pub fn open<'a>(a: &'a Args, em: &'a mut Emitter, role: &'static str) -> Result<Node<'a>, String> {
    let t0 = Instant::now();
    let store = open_store(a, em)?;
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
        applied: 0,
        until_seen: None,
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
    Ok(n)
}

impl Node<'_> {
    pub fn lease(&self) -> LeaseState {
        self.store.lease_state()
    }

    /// Emits what changed in memory: the lease, the read-only reason, heartbeats. No I/O.
    pub fn observe(&mut self, why: &str) {
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
                    let kind = kind_of(&c);
                    self.applied += 1;
                    if self.until_seen.is_none() && self.a.until_kind.as_deref() == Some(&kind) {
                        self.until_seen = Some(c.tx.tx_id.clone());
                    }
                    self.em.emit(
                        "applied",
                        Some(&l),
                        json!({
                            "txId": c.tx.tx_id, "seq": c.rev, "origin": c.tx.author,
                            "branch": c.tx.branch, "kind": kind,
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
                StoreEvent::Status(_) | StoreEvent::Collect(_) => {}
            }
        }
    }

    pub fn tick(&mut self) -> Result<(), String> {
        let before = self.applied;
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
        // A follower's view after each import that changed it (the `history` row compares
        // the last one with the merger's).
        if self.role == "follower" && self.applied > before {
            let h = hashes::of(&self.store, &self.db);
            let l = self.lease();
            self.em.emit("hashes", Some(&l), h);
        }
        Ok(())
    }

    /// A fresh txId for this node's next write.
    pub fn next_tx_id(&mut self) -> String {
        self.commits += 1;
        format!("{}.{}.{}", self.a.label, std::process::id(), self.commits)
    }

    /// This node's author string.
    pub fn author(&self) -> String {
        format!("xhost:{}:{}", self.a.label, self.store.server_id())
    }

    fn commit(&mut self) {
        let tx_id = self.next_tx_id();
        let op = asset_op(&tx_id);
        self.commit_on(tx_id, MAIN, vec![op], json!({}));
    }

    /// Commits `ops` on `branch` as `tx_id` and logs it (`extra`: more fields of the event).
    pub fn commit_on(
        &mut self,
        tx_id: String,
        branch: &str,
        ops: Vec<Op>,
        extra: Value,
    ) -> Option<CommittedTx> {
        let tx = Tx {
            tx_id: tx_id.clone(),
            author: self.author(),
            branch: branch.into(),
            base_rev: self.store.head(branch),
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            via: None,
            ops,
        };
        let before = self.lease();
        let t = Instant::now();
        let r = self.store.commit(tx);
        let ms = t.elapsed().as_millis() as u64;
        let out = match r {
            Ok(c) => {
                let ids: Vec<String> = c.bindings.values().cloned().collect();
                self.accepted_event(&c, &before, ms, ids, extra);
                Some(c)
            }
            Err(e) => {
                self.refused_event(&tx_id, branch, e, ms);
                None
            }
        };
        self.observe("commit");
        out
    }

    /// Logs an accepted record (`ids`: the entity ids it bound, for the analyser's id-reuse
    /// check; none for copies, which keep their origin's ids).
    pub fn accepted_event(
        &mut self,
        c: &CommittedTx,
        before: &LeaseState,
        ms: u64,
        ids: Vec<String>,
        extra: Value,
    ) {
        let row = self.db.row(&c.tx.tx_id);
        self.accepted.push(c.tx.tx_id.clone());
        let l = self.lease();
        let mut ev = json!({
            "ok": true, "txId": c.tx.tx_id, "seq": c.rev, "branch": c.tx.branch,
            "kind": kind_of(c),
            "txHash": row.as_ref().map(|r| r.tx_hash.clone()),
            "rowEpoch": row.as_ref().map(|r| r.epoch),
            "leaseBefore": lease_parts(before).1, "ms": ms, "ids": ids,
        });
        if let (Some(o), Value::Object(x)) = (ev.as_object_mut(), extra) {
            o.extend(x);
        }
        self.em.emit("commit", Some(&l), ev);
    }

    /// Logs a refused write.
    pub fn refused_event(&mut self, tx_id: &str, branch: &str, e: fin_store::StoreError, ms: u64) {
        let l = self.lease();
        let e: CoreError = e.into();
        let reason = e.details.get("reason").cloned();
        self.em.emit(
            "commit",
            Some(&l),
            json!({ "ok": false, "txId": tx_id, "branch": branch, "code": e.code, "reason": reason, "message": e.message, "ms": ms }),
        );
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
        let mut until_at: Option<Instant> = None;
        while Instant::now() < end {
            let now = Instant::now();
            // `--until-kind K`: once a record of kind K applied, `--settle-ms` more, then one
            // full follow cycle (a fresh poll and a tick), and stop.
            if self.until_seen.is_some() {
                let at = *until_at.get_or_insert(now + Duration::from_millis(a.settle_ms));
                if now >= at {
                    self.store.poll_now_for_test();
                    self.tick()?;
                    let l = self.lease();
                    let h = hashes::of(&self.store, &self.db);
                    self.em.emit(
                        "followed",
                        Some(&l),
                        json!({ "until": self.until_seen, "hashes": h }),
                    );
                    break;
                }
            }
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

    /// Final report: what became of every accepted commit, then close (or, without `close`,
    /// stop as a crash would: no export, the lease left to go stale), then the chain.
    pub fn finish(self, close: bool) -> Result<(), String> {
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
                "branches": status.branches, "hashes": hashes::of(&self.store, &self.db),
            }),
        );
        let Node { em, store, db, .. } = self;
        if close {
            match store.close() {
                Ok(r) => em.emit(
                    "closed",
                    Some(&l),
                    json!({ "segments": r.segments_written, "records": r.records, "error": r.error }),
                ),
                Err(e) => em.emit("closed", Some(&l), json!({ "error": e.to_string() })),
            }
        } else {
            // Nothing is released: the process ends with the lease and its files as they are.
            std::mem::forget(store);
            em.emit("stopped", Some(&l), json!({}));
        }
        em.emit(
            "chain",
            None,
            json!({ "rows": db.chain()?, "all": db.all_rows()?, "quarantine": db.quarantine()? }),
        );
        Ok(())
    }
}
