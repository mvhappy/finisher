//! Long history chains (external review of 2026-11-09, #15 and #16): thousands of undo/redo
//! cycles, and long or looping origin chains, plan on a 2 MiB stack (a libuv worker's, where
//! fin-node runs history as napi async tasks) without recursing per record.

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::history::{LogView, Roots, root};
use fin_core::{CoreError, ItemSplit, Link, Op, Rational};

const CYCLES: usize = 5_000;

/// Runs `f` on a thread with a 2 MiB stack and returns what it returns.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .expect("the thread finished (no stack overflow)")
}

/// `n` undo/redo cycles of `author`'s top unit `unit` (whose inverse is `inverse`) on `b`,
/// planned (`u<i>`, `r<i>`).
fn cycles(log: &mut MemLog, b: &str, author: &str, n: usize) {
    for i in 0..n {
        log.undo(b, author, &format!("u{i}")).unwrap();
        log.redo(b, author, &format!("r{i}")).unwrap();
    }
}

/// The records [`cycles`] writes, appended without planning each: planning one record reads
/// the branch's records (linear), so planning 10,000 of them is quadratic, while the depth
/// this file is about is what one plan walks. `synthetic_cycles_are_the_planned_records`
/// checks they are the same records.
fn fast_cycles(log: &mut MemLog, b: &str, author: &str, unit: &str, inverse: Vec<Op>, n: usize) {
    let (mut unit, mut inverse) = (unit.to_string(), inverse);
    for i in 0..n {
        let undoes = vec![unit.clone()];
        let u = log.append_link(b, &format!("u{i}"), author, Link::Undo { undoes }, inverse);
        let redoes = u.record.tx_id.clone();
        let ops = u.record.inverse.clone();
        let r = log.append_link(b, &format!("r{i}"), author, Link::Redo { redoes }, ops);
        unit = r.record.tx_id.clone();
        inverse = r.record.inverse.clone();
    }
}

fn split(item: &str, at: i64) -> Op {
    ItemSplit {
        item: item.into(),
        offset: secs(at),
        id: "$r".into(),
    }
    .into()
}

fn items(log: &MemLog, b: &str) -> Vec<(String, Rational, Rational)> {
    items_of(&log.state(b))
}

fn items_of(p: &fin_core::Project) -> Vec<(String, Rational, Rational)> {
    p.timelines[0].tracks[0]
        .items
        .iter()
        .map(|i| (i.id.clone(), i.start, i.duration))
        .collect()
}

fn genesis() -> Vec<(String, Rational, Rational)> {
    items_of(&with_clips(&[10, 10, 10]))
}

#[test]
fn synthetic_cycles_are_the_planned_records() {
    for edit in [delete("c1"), split("c2", 4)] {
        let mut planned = MemLog::new(with_clips(&[10, 10, 10]));
        let mut fast = MemLog::new(with_clips(&[10, 10, 10]));
        planned.commit("main", "e1", "carol", vec![edit.clone()]);
        let e = fast.commit("main", "e1", "carol", vec![edit]);
        cycles(&mut planned, "main", "carol", 3);
        fast_cycles(
            &mut fast,
            "main",
            "carol",
            "e1",
            e.record.inverse.clone(),
            3,
        );
        for i in 0..3 {
            for id in [format!("u{i}"), format!("r{i}")] {
                let (p, f) = (
                    planned.find(&id).unwrap().unwrap(),
                    fast.find(&id).unwrap().unwrap(),
                );
                assert_eq!(p.record.link().unwrap(), f.record.link().unwrap(), "{id}");
                assert_eq!(p.record.intent.ops, f.record.intent.ops, "{id}");
                assert_eq!(p.record.inverse, f.record.inverse, "{id}");
            }
        }
        assert_eq!(
            planned.stacks("main", "carol"),
            fast.stacks("main", "carol")
        );
        assert_eq!(items(&planned, "main"), items(&fast, "main"));
    }
}

/// Plans on top of 5,000 cycles of `edit`'s unit: undo, redo, undo again, `undo --to` a
/// checkpoint before them, and a cherry-pick of the newest redo.
fn plan_on_a_long_chain(edit: Op) {
    on_small_stack(move || {
        let mut log = MemLog::new(with_clips(&[10, 10, 10]));
        let e = log.commit("main", "e1", "carol", vec![edit]);
        let edited = items(&log, "main");
        log.fork("side", "main", 0);
        let cp = log.checkpoint("main", "cp", "carol", "before");
        fast_cycles(
            &mut log,
            "main",
            "carol",
            "e1",
            e.record.inverse.clone(),
            CYCLES,
        );
        assert_eq!(items(&log, "main"), edited);
        log.undo("main", "carol", "last-u").unwrap();
        assert_eq!(items(&log, "main"), genesis());
        log.redo("main", "carol", "last-r").unwrap();
        assert_eq!(items(&log, "main"), edited);
        // The unit was made before the checkpoint: nothing to take (Made::redo walks every
        // redo since).
        let to = log.undo_to("main", "carol", cp, "to").unwrap_err();
        assert_eq!(to.code, "NOTHING_TO_UNDO", "{to:?}");
        // A redo copied without its undo: refused, after judging its marks (absorbs::of_ops).
        let last = format!("r{}", CYCLES - 1);
        let pick = log.pick(&last, "side", "dave", "k1").unwrap_err();
        assert_eq!(pick.details["reason"], "DEPENDS_ON_SKIPPED", "{pick:?}");
        log.undo("main", "carol", "final").unwrap();
        assert_eq!(items(&log, "main"), genesis());
    });
}

#[test]
fn plain_edit_survives_5000_undo_redo_cycles() {
    plan_on_a_long_chain(delete("c1"));
}

#[test]
fn split_survives_5000_undo_redo_cycles() {
    // A split's undo carries absorb marks (history/absorbs.rs): judging them walks every
    // earlier record of the chain.
    plan_on_a_long_chain(split("c2", 4));
}

#[test]
fn merge_survives_5000_undo_redo_cycles_then_reverts() {
    on_small_stack(|| {
        let mut log = MemLog::new(with_clips(&[10, 10, 10]));
        log.fork("br1", "main", 0);
        let op = insert("$x", "t1", secs(30), "a1", Rational::zero(), secs(2));
        log.commit("br1", "e1", "alice", vec![op]);
        log.merge("br1", "main", "carol", "m", false).unwrap();
        let merged = items(&log, "main");
        let copy = log.merge_group("m").unwrap().remove(0);
        fast_cycles(
            &mut log,
            "main",
            "carol",
            "m",
            copy.record.inverse.clone(),
            CYCLES,
        );
        assert_eq!(items(&log, "main"), merged);
        log.undo("main", "carol", "last-u").unwrap();
        log.redo("main", "carol", "last-r").unwrap();
        // Revert judges whether the group is still in effect through every undo and redo.
        assert!(log.revert("m", "main", "carol", "v1").unwrap().is_some());
        assert_eq!(items(&log, "main"), genesis());
    });
}

/// `n` merge links: `c1` copies `e` onto `b1`, `c<k>` copies `c<k-1>` onto `b<k>`.
fn merge_chain(n: usize) -> (MemLog, fin_core::history::LogRec) {
    let mut log = MemLog::new(with_clips(&[10, 10, 10]));
    let e = log.commit("main", "e", "alice", vec![delete("c1")]);
    let mut prev = e;
    for k in 1..=n {
        let b = format!("b{k}");
        log.fork(&b, "main", 0);
        let link = Link::Merge {
            merge_id: format!("m{k}"),
            from: if k == 1 {
                "main".into()
            } else {
                format!("b{}", k - 1)
            },
            origin: prev.record.tx_id.clone(),
        };
        prev = log.copy(&b, &format!("c{k}"), "carol", link, &prev);
    }
    (log, prev)
}

#[test]
fn origin_root_past_a_thousand_merge_links() {
    on_small_stack(|| {
        let (mut log, last) = merge_chain(1_002);
        let (id, held) = root(&log, &last).unwrap();
        assert_eq!(id, "e");
        assert_eq!(held.unwrap().record.tx_id, "e");
        // The copy's root is already on main: nothing to pick (it was CONFLICT REMOVED with
        // the intermediate root).
        let pick = log.pick(&last.record.tx_id, "main", "dave", "k1").unwrap();
        assert!(pick.is_none());
    });
}

fn merge_link(origin: &str) -> Link {
    Link::Merge {
        merge_id: format!("m-{origin}"),
        from: "elsewhere".into(),
        origin: origin.into(),
    }
}

fn db_error(e: CoreError, reason: &str) {
    assert_eq!(e.code, "DB_ERROR", "{e:?}");
    assert_eq!(e.details["reason"], reason, "{e:?}");
}

#[test]
fn origin_cycle_is_a_typed_error() {
    on_small_stack(|| {
        let mut log = MemLog::new(with_clips(&[10, 10, 10]));
        log.fork("b", "main", 0);
        // Stored links that loop: x copies y, y copies x.
        let x = log.append_link("b", "x", "carol", merge_link("y"), vec![delete("c1")]);
        log.append_link("b", "y", "carol", merge_link("x"), vec![delete("c2")]);
        db_error(root(&log, &x).unwrap_err(), "originCycle");
        db_error(
            log.pick("x", "main", "dave", "k1").unwrap_err(),
            "originCycle",
        );
        // A record naming itself is a loop too.
        let z = log.append_link("b", "z", "carol", merge_link("z"), vec![delete("c3")]);
        db_error(root(&log, &z).unwrap_err(), "originCycle");
    });
}

#[test]
fn merge_roots_through_a_long_undo_redo_chain() {
    // A merge judges each source record's units through every undo and redo before it
    // (origin.rs `Ties`): the newest redo of 5,000 cycles of an undone merge from main undoes
    // main's own record.
    on_small_stack(|| {
        let mut log = MemLog::new(with_clips(&[10, 10, 10]));
        log.commit("main", "e1", "carol", vec![delete("c1")]);
        log.fork("work", "main", 1);
        log.fork("other", "main", 0);
        log.commit("other", "o1", "carol", vec![delete("c2")]);
        log.merge("other", "work", "carol", "m", false).unwrap();
        let copy = log.merge_group("m").unwrap().remove(0);
        fast_cycles(
            &mut log,
            "work",
            "carol",
            "m",
            copy.record.inverse.clone(),
            CYCLES,
        );
        let last = log.find(&format!("r{}", CYCLES - 1)).unwrap().unwrap();
        let roots = |target: &str| {
            let l = log.lineage(target).unwrap();
            Roots::new(&log, &l, std::slice::from_ref(&last), &[]).unwrap()
        };
        // On `other` (where o1 was made) the redo undoes a target record; on main it does not.
        assert_eq!(roots("other").undoes_target, vec![true]);
        assert_eq!(roots("main").undoes_target, vec![false]);
    });
}
