//! `brancher` and `merger` (plan 4B Task 23, the `history` row): history records between
//! servers on a share.
//!
//! - `brancher`: holds the lease, commits a clip on `main`, (with `--plain-ms T`, edits
//!   `main` only for T ms first: the mixed-version row), creates branch `--branch` (default
//!   `x`), commits `--edits` edits alternating between it and `main` (one same-edge conflict:
//!   both trim the clip's end), waits until everything is exported, then stops without
//!   closing (the lease is left to go stale, as on a crash).
//! - `merger`: opens, waits for the takeover (the brancher's lease going stale) and for the
//!   heads `--expect id=seq,…`, merges the branch into `main` with `--skip-conflicting`,
//!   waits for the export, emits `merged` (`{merged, skipped, stateHash, indexHash, heads}`),
//!   and closes.

use std::time::{Duration, Instant};

use fin_core::{Edge, Rational};
use fin_store::lease::LeaseState;
use fin_store::{BranchCreate, MAIN, MergeReq};
use serde_json::json;

use crate::Args;
use crate::emit::Emitter;
use crate::hashes;
use crate::node::{self, Node};
use crate::ops::asset_op;

/// Ticks (and samples the share's clock) until `until`.
struct Pacer {
    next_clock: Instant,
}

impl Pacer {
    fn new() -> Pacer {
        Pacer {
            next_clock: Instant::now(),
        }
    }

    fn step(&mut self, n: &mut Node) -> Result<(), String> {
        if Instant::now() >= self.next_clock {
            let l = n.lease();
            n.em.clock_sample(&n.a.dir, Some(&l));
            self.next_clock = Instant::now() + Duration::from_millis(n.a.clock_every_ms);
        }
        n.tick()
    }

    fn until(&mut self, n: &mut Node, until: Instant) -> Result<(), String> {
        loop {
            self.step(n)?;
            let now = Instant::now();
            if now >= until {
                return Ok(());
            }
            std::thread::sleep((until - now).min(Duration::from_millis(n.a.tick_ms)));
        }
    }

    /// Ticks until this store holds the lease and may write; `Err` at `end`.
    fn held(&mut self, n: &mut Node, end: Instant) -> Result<u64, String> {
        let t = Instant::now();
        loop {
            self.step(n)?;
            if matches!(n.lease(), LeaseState::Held { .. }) && n.store.read_only().is_none() {
                let ms = t.elapsed().as_millis() as u64;
                let l = n.lease();
                n.em.emit("writable", Some(&l), json!({ "waitMs": ms }));
                return Ok(ms);
            }
            if Instant::now() >= end {
                return Err("never held the lease".into());
            }
            std::thread::sleep(Duration::from_millis(n.a.tick_ms));
        }
    }

    /// Ticks until every branch is exported through its head; `false` at `end`.
    fn exported(&mut self, n: &mut Node, end: Instant) -> Result<bool, String> {
        loop {
            self.step(n)?;
            let st = n.store.status();
            let done = st.branches.iter().all(|b| b.exported_through >= b.head);
            if done || Instant::now() >= end {
                let l = n.lease();
                n.em.emit(
                    "exported",
                    Some(&l),
                    json!({ "complete": done, "branches": st.branches }),
                );
                return Ok(done);
            }
            std::thread::sleep(Duration::from_millis(n.a.tick_ms));
        }
    }
}

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

pub fn brancher(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let mut n = node::open(a, em, "brancher")?;
    let end = Instant::now() + Duration::from_secs_f64(a.secs);
    let every = Duration::from_millis(a.commit_every_ms);
    let mut p = Pacer::new();
    p.held(&mut n, end)?;
    // The clip both sides trim.
    let id = n.next_tx_id();
    let op = asset_op(&id);
    let c = n
        .commit_on(id, MAIN, vec![op], json!({ "phase": "setup" }))
        .ok_or("setup asset refused")?;
    let asset = c.bindings.get("$a").cloned().ok_or("no asset binding")?;
    let id = n.next_tx_id();
    let op = fin_core::ItemInsert {
        id: "$c".into(),
        track: "t1".into(),
        at: secs(0),
        duration: secs(5),
        mode: fin_core::InsertMode::Ripple,
        body: fin_core::ItemBody::Clip {
            asset,
            source_in: secs(0),
            speed: fin_core::Rational::from_int(1),
        },
    }
    .into();
    let c = n
        .commit_on(id, MAIN, vec![op], json!({ "phase": "setup" }))
        .ok_or("setup clip refused")?;
    let clip = c.bindings.get("$c").cloned().ok_or("no clip binding")?;
    // Edit-only phase (mixed versions): a 4A follower follows these.
    let plain_end = Instant::now() + Duration::from_millis(a.plain_ms);
    while Instant::now() < plain_end {
        let id = n.next_tx_id();
        let op = asset_op(&id);
        n.commit_on(id, MAIN, vec![op], json!({ "phase": "plain" }));
        p.until(&mut n, Instant::now() + every)?;
    }
    // The branch.
    let id = n.next_tx_id();
    let before = n.lease();
    let t = Instant::now();
    let r = n.store.create_branch(BranchCreate {
        tx_id: id.clone(),
        author: n.author(),
        name: Some(a.branch.clone()),
        from: MAIN.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    });
    let ms = t.elapsed().as_millis() as u64;
    let x = match r {
        Ok(b) => {
            let extra = json!({ "phase": "branch", "branchId": b.branch.id, "name": a.branch });
            n.accepted_event(&b.record, &before, ms, Vec::new(), extra);
            b.branch.id
        }
        Err(e) => {
            n.refused_event(&id, MAIN, e, ms);
            return Err("branch create refused".into());
        }
    };
    p.until(&mut n, Instant::now() + every)?;
    // Edits alternating between the branch and main; edits 2 (branch) and 3 (main) trim the
    // same edge of the clip: the branch's trim is the conflict the merge skips.
    for i in 0..a.edits {
        let on = if i % 2 == 0 {
            x.clone()
        } else {
            MAIN.to_string()
        };
        let id = n.next_tx_id();
        let (op, conflict) = match i {
            2 | 3 => (
                fin_core::ItemTrim {
                    item: clip.clone(),
                    edge: Edge::End,
                    delta: secs(if i == 2 { -1 } else { -2 }),
                    mode: fin_core::TrimMode::Ripple,
                }
                .into(),
                true,
            ),
            _ => (asset_op(&id), false),
        };
        n.commit_on(
            id,
            &on,
            vec![op],
            json!({ "phase": "edits", "conflict": conflict }),
        );
        p.until(&mut n, Instant::now() + every)?;
    }
    p.exported(&mut n, end)?;
    n.finish(false)
}

pub fn merger(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let mut n = node::open(a, em, "merger")?;
    let end = Instant::now() + Duration::from_secs_f64(a.secs);
    let mut p = Pacer::new();
    p.held(&mut n, end)?;
    // Everything the brancher exported, before merging.
    let catch_up = (Instant::now() + Duration::from_secs(30)).min(end);
    let reached = loop {
        let behind: Vec<_> = a
            .expect
            .iter()
            .filter(|(b, seq)| n.store.head(b) < *seq)
            .map(|(b, seq)| json!({ "branch": b, "expected": seq, "head": n.store.head(b) }))
            .collect();
        if behind.is_empty() || Instant::now() >= catch_up {
            let l = n.lease();
            n.em.emit("caughtUp", Some(&l), json!({ "behind": behind }));
            break behind.is_empty();
        }
        n.store.poll_now_for_test();
        p.until(&mut n, Instant::now() + Duration::from_millis(500))?;
    };
    let id = format!("{}.{}.merge", a.label, std::process::id());
    let before = n.lease();
    let t = Instant::now();
    let r = n.store.merge(MergeReq {
        tx_id: id.clone(),
        author: n.author(),
        from: a.branch.clone(),
        into: MAIN.into(),
        skip_conflicting: true,
        dry_run: false,
        args_hash: None,
    });
    let ms = t.elapsed().as_millis() as u64;
    let rep = match r {
        Ok(rep) => rep,
        Err(e) => {
            n.refused_event(&id, MAIN, e, ms);
            return Err("merge refused".into());
        }
    };
    for c in &rep.committed {
        n.accepted_event(c, &before, ms, Vec::new(), json!({ "phase": "merge" }));
    }
    let exported = p.exported(&mut n, end)?;
    let h = hashes::of(&n.store, &n.db);
    let l = n.lease();
    n.em.emit(
        "merged",
        Some(&l),
        json!({
            "mergeId": rep.merge_id, "merged": rep.merged.len(), "origins": rep.merged,
            "skipped": rep.skipped, "excluded": rep.excluded, "sourceState": rep.source_state,
            "caughtUp": reached, "exported": exported, "ms": ms,
            "stateHash": h["stateHash"], "indexHash": h["indexHash"], "heads": h["heads"],
            "hashes": h,
        }),
    );
    n.finish(true)
}
