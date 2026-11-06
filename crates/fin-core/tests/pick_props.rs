//! Chains of merge, revert, undo and redo (plan 4B Task 14 review): whatever chain follows a
//! merge on the target, a later merge or cherry-pick of a new source edit lands where it does
//! on the source, and a stale edit over the chain lands where it does over the chain's net
//! effect alone.

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::{Op, Project, Rational};
use proptest::prelude::*;

/// `PROPTEST_CASES` overrides the per-block default.
fn config(default: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    ProptestConfig {
        cases,
        ..ProptestConfig::default()
    }
}

/// `c1` [0,10), `c2` [10,20), `c3` [20,30) on `t1`, asset `a1`.
fn genesis() -> Project {
    with_clips(&[10, 10, 10])
}

fn ins(id: &str, at: Rational, dur: i64) -> Op {
    insert(id, "t1", at, "a1", Rational::zero(), secs(dur))
}

fn order(p: &Project) -> Vec<String> {
    starts(p, 0).into_iter().map(|(id, _)| id).collect()
}

/// `(start, duration)` of every clip on `t1`: the layout, ids aside.
fn layout(p: &Project) -> Vec<(Rational, Rational)> {
    p.timeline.tracks[0]
        .items
        .iter()
        .map(|c| (c.start, c.duration))
        .collect()
}

/// A clip edge on `b`: the start of clip `k`, or the track's end.
fn edge(log: &MemLog, b: &str, k: usize) -> Rational {
    let p = log.state(b);
    let items = &p.timeline.tracks[0].items;
    items.get(k % (items.len() + 1)).map_or_else(
        || {
            items
                .last()
                .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap())
        },
        |c| c.start,
    )
}

#[derive(Debug, Clone, Copy)]
enum Act {
    Revert,
    Undo,
    Redo,
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![Just(Act::Revert), Just(Act::Undo), Just(Act::Redo)]
}

/// br1 inserts `c4` at edge `first` and deletes nothing; merged into main by carol as `m`;
/// then `chain` on main by carol (steps that do not apply are left out). Whether the merge's
/// effect is on main at the end.
fn build(first: usize, chain: &[Act]) -> (MemLog, bool) {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    let at = edge(&log, "br1", first);
    log.commit("br1", "e1", "alice", vec![ins("$p", at, 2)]);
    log.merge("br1", "main", "carol", "m", false).unwrap();
    for (n, a) in chain.iter().enumerate() {
        let id = format!("h{n}");
        let _ = match a {
            Act::Revert => log.revert("m", "main", "carol", &id).map(|_| ()),
            Act::Undo => log.undo("main", "carol", &id).map(|_| ()),
            Act::Redo => log.redo("main", "carol", &id).map(|_| ()),
        };
    }
    let on = log.state("main") == log.state("br1");
    if !on {
        let mut without = log.state("br1");
        without.timeline.tracks[0].items.retain(|c| c.id != "c4");
        assert_eq!(order(&log.state("main")), order(&without));
    }
    (log, on)
}

proptest! {
    #![proptest_config(config(64))]

    #[test]
    fn merge_and_pick_after_any_chain_keep_the_source_order(
        first in 0usize..4,
        chain in prop::collection::vec(act(), 0..6),
        k in 0usize..6,
        pick in any::<bool>(),
    ) {
        let (mut log, on) = build(first, &chain);
        let at = edge(&log, "br1", k);
        log.commit("br1", "e3", "alice", vec![ins("$q", at, 1)]);
        let new_id = order(&log.state("br1")).into_iter().find(|c| c == "c5");
        prop_assert!(new_id.is_some());
        if pick {
            log.pick("e3", "main", "dave", "k1").unwrap().unwrap();
        } else {
            log.merge("br1", "main", "carol", "m2", false).unwrap();
        }
        let mut want = order(&log.state("br1"));
        if !on {
            want.retain(|c| c != "c4");
        }
        prop_assert_eq!(order(&log.state("main")), want);
        assert_well_formed(&log.state("main"));
    }

    #[test]
    fn stale_edit_over_any_chain_lands_as_over_its_net_effect(
        first in 0usize..4,
        chain in prop::collection::vec(act(), 0..6),
        k in 0usize..4,
    ) {
        let (mut log, on) = build(first, &chain);
        let at = edge(&MemLog::new(genesis()), "main", k);
        let w = vec![ins("$w", at, 5)];
        log.commit_at("main", "w1", "bob", 0, w.clone()).unwrap();
        // The same stale edit over the merge alone (in effect) or over nothing.
        let (mut net, _) = build(first, &[]);
        if !on {
            net = MemLog::new(genesis());
            net.fork("br1", "main", 0);
        }
        net.commit_at("main", "w1", "bob", 0, w).unwrap();
        prop_assert_eq!(layout(&log.state("main")), layout(&net.state("main")));
    }
}
