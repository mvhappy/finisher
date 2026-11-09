//! Durable rejections (sub-project 5 spec §8.3; plan 5E Decisions 1–3): what a lineage's
//! `drop` records leave dropped, and how merges, the merged state, cherry-picks and the undo
//! stacks treat a dropped origin.

mod common;

use std::collections::BTreeSet;

use common::memlog::MemLog;
use common::*;
use fin_core::history::{DropMark, LogView, dropped_for, left_to_merge, plan_merge};
use fin_core::rebase::reasons;
use fin_core::{Edge, Op, Project, Rational};

/// `c1` [0,10), `c2` [10,20), `c3` [20,30) on `t1`, asset `a1`.
fn genesis() -> Project {
    with_clips(&[10, 10, 10])
}

fn ins(at: i64, dur: i64) -> Op {
    insert("$x", "t1", secs(at), "a1", Rational::zero(), secs(dur))
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

fn dropped(log: &MemLog, b: &str) -> BTreeSet<String> {
    dropped_for(log, &log.lineage(b).unwrap()).unwrap()
}

/// `br1` forked from `main` at 0 with three independent edits `e1`–`e3` (each deletes one
/// of `c1`–`c3`).
fn three_edits() -> MemLog {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    for (tx, clip) in [("e1", "c1"), ("e2", "c2"), ("e3", "c3")] {
        log.commit("br1", tx, "alice", vec![delete(clip)]);
    }
    log
}

fn excluded(plan: &fin_core::history::MergePlan) -> Vec<(&str, &str)> {
    plan.excluded
        .iter()
        .map(|c| (c.tx_id.as_str(), c.reason.as_str()))
        .collect()
}

/// The newest record on the lineage naming an origin decides; an ancestor's records up to the
/// fork come before the branch's own.
#[test]
fn newest_record_wins() {
    let mut log = three_edits();
    log.drop_rec("main", "d1", "br1", &["e1", "e2"], &[]);
    assert_eq!(dropped(&log, "main"), set(&["e1", "e2"]));
    log.drop_rec("main", "d2", "br1", &[], &["e1"]);
    assert_eq!(dropped(&log, "main"), set(&["e2"]));
    log.drop_rec("main", "d3", "br1", &["e1"], &["e2"]);
    assert_eq!(dropped(&log, "main"), set(&["e1"]));
    // An undrop of what was never dropped changes nothing.
    log.drop_rec("main", "d4", "br1", &[], &["e3"]);
    assert_eq!(dropped(&log, "main"), set(&["e1"]));

    // A child's own records come after everything it inherits.
    log.fork("ch", "main", log.head_seq("main"));
    log.drop_rec("ch", "d5", "br1", &["e3"], &["e1"]);
    assert_eq!(dropped(&log, "ch"), set(&["e3"]));
    assert_eq!(dropped(&log, "main"), set(&["e1"]));

    // `drops_on` lists every mark in lineage order, oldest first.
    let marks = log.drops_on(&log.lineage("ch").unwrap()).unwrap();
    let seen: Vec<(&str, u64, &str, bool)> = marks
        .iter()
        .map(|m: &DropMark| (m.branch.as_str(), m.seq, m.origin.as_str(), m.dropped))
        .collect();
    assert_eq!(
        seen,
        [
            ("main", 1, "e1", true),
            ("main", 1, "e2", true),
            ("main", 2, "e1", false),
            ("main", 3, "e1", true),
            ("main", 3, "e2", false),
            ("main", 4, "e3", false),
            ("ch", 5, "e3", true),
            ("ch", 5, "e1", false),
        ]
    );
}

/// A branch forked after a record inherits it; one written on the parent after the fork does
/// not reach the child.
#[test]
fn child_inherits_before_fork_only() {
    let mut log = three_edits();
    log.drop_rec("main", "d1", "br1", &["e1"], &[]);
    log.fork("ch", "main", log.head_seq("main"));
    log.fork("early", "main", 0);
    log.drop_rec("main", "d2", "br1", &["e2"], &[]);
    assert_eq!(dropped(&log, "main"), set(&["e1", "e2"]));
    assert_eq!(dropped(&log, "ch"), set(&["e1"]));
    assert_eq!(dropped(&log, "early"), set(&[]));
    // Grandchildren inherit through both forks.
    log.fork("gc", "ch", log.head_seq("ch"));
    assert_eq!(dropped(&log, "gc"), set(&["e1"]));

    let plan = log.merge("br1", "ch", "carol", "m", false).unwrap();
    assert_eq!(excluded(&plan), [("e1", reasons::DROPPED)]);
    assert_eq!(plan.merged, ["e2", "e3"]);
    let plan = log.merge("br1", "early", "carol", "m2", false).unwrap();
    assert!(plan.excluded.is_empty());
    assert_eq!(plan.merged, ["e1", "e2", "e3"]);
}

/// An undrop makes the origin a candidate again: a later plain merge copies it.
#[test]
fn undrop_restores_candidate() {
    let mut log = three_edits();
    log.drop_rec("main", "d1", "br1", &["e2"], &[]);
    let plan = log.merge("br1", "main", "carol", "m1", false).unwrap();
    assert_eq!(plan.merged, ["e1", "e3"]);
    // Merging again offers nothing new.
    let plan = log.merge("br1", "main", "carol", "m2", false).unwrap();
    assert!(plan.merged.is_empty());
    assert_eq!(excluded(&plan), [("e2", reasons::DROPPED)]);
    log.drop_rec("main", "d2", "br1", &[], &["e2"]);
    let plan = log.merge("br1", "main", "carol", "m3", false).unwrap();
    assert!(plan.excluded.is_empty());
    assert_eq!(plan.merged, ["e2"]);
    assert!(
        starts(&log.state("main"), 0).is_empty(),
        "all three clips gone"
    );
}

/// A dropped origin is left out with `DROPPED` in `excluded` (not a conflict), also when the
/// source holds it as a copy (judged by origin root).
#[test]
fn dropped_is_excluded_with_reason() {
    let mut log = three_edits();
    log.drop_rec("main", "d1", "br1", &["e2"], &[]);
    let plan = log.merge("br1", "main", "carol", "m1", false).unwrap();
    assert_eq!(excluded(&plan), [("e2", reasons::DROPPED)]);
    assert!(plan.conflicts.is_empty());
    assert_eq!(plan.merged, ["e1", "e3"]);
    assert_eq!(
        plan.excluded[0].fix,
        fin_core::Conflict::new("x", 0, reasons::DROPPED, None).fix
    );
    assert!(!plan.excluded[0].fix.is_empty());

    // br2 brought br1's records in by a merge: the copy of e2 has root e2, so it is dropped.
    let mut log = three_edits();
    log.fork("br2", "main", 0);
    log.merge("br1", "br2", "carol", "mb", false).unwrap();
    log.drop_rec("main", "d1", "br1", &["e2"], &[]);
    let plan = log.merge("br2", "main", "carol", "m1", false).unwrap();
    assert_eq!(excluded(&plan), [("mb-2", reasons::DROPPED)]);
    assert_eq!(plan.merged, ["mb-1", "mb-3"]);
}

/// What depends on a dropped origin is `DEPENDS_ON_SKIPPED` (a conflict): an edit of what it
/// inserted, and an undo of it.
#[test]
fn dependent_of_dropped_is_depends_on_skipped() {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "alice", vec![ins(30, 2)]);
    log.commit("br1", "e2", "alice", vec![trim("c4", Edge::End, secs(-1))]);
    log.commit("br1", "e3", "alice", vec![delete("c1")]);
    log.drop_rec("main", "d1", "br1", &["e1"], &[]);
    let err = log.merge("br1", "main", "carol", "m1", false).unwrap_err();
    assert_eq!(err.code, "CONFLICT");
    let conflicts = err.details["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(conflicts[0]["txId"], "e2");
    assert_eq!(conflicts[0]["reason"], reasons::DEPENDS_ON_SKIPPED);
    let plan = log.merge("br1", "main", "carol", "m2", true).unwrap();
    assert_eq!(excluded(&plan), [("e1", reasons::DROPPED)]);
    assert_eq!(plan.merged, ["e3"]);

    // An undo of a dropped unit would apply an inverse the target never had the original of.
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "alice", vec![delete("c1")]);
    log.undo("br1", "alice", "u1").unwrap();
    log.drop_rec("main", "d1", "br1", &["e1"], &[]);
    let plan = log.merge("br1", "main", "carol", "m1", true).unwrap();
    assert_eq!(excluded(&plan), [("e1", reasons::DROPPED)]);
    assert_eq!(plan.conflicts.len(), 1);
    assert_eq!(plan.conflicts[0].tx_id, "u1");
    assert_eq!(plan.conflicts[0].reason, reasons::DEPENDS_ON_SKIPPED);
    assert_eq!(plan.conflicts[0].against_tx.as_deref(), Some("e1"));
}

/// An explicit cherry-pick of a dropped origin is allowed and does not undrop it.
#[test]
fn cherry_pick_ignores_drops() {
    let mut log = three_edits();
    log.drop_rec("main", "d1", "br1", &["e2"], &[]);
    let k = log.pick("e2", "main", "dave", "k1").unwrap();
    assert!(k.is_some(), "the pick is made");
    assert_eq!(dropped(&log, "main"), set(&["e2"]));
    // The target holds e2 now: a merge leaves it out silently, as already present.
    let plan = log.merge("br1", "main", "carol", "m1", false).unwrap();
    assert!(plan.excluded.is_empty());
    assert_eq!(plan.merged, ["e1", "e3"]);
}

/// A dropped origin is not left to merge, so a branch whose rest is merged or dropped is merged.
#[test]
fn left_to_merge_skips_dropped() {
    let mut log = three_edits();
    assert_eq!(
        left_to_merge(&log, "br1", "main").unwrap(),
        ["e1", "e2", "e3"]
    );
    log.drop_rec("main", "d1", "br1", &["e2"], &[]);
    assert_eq!(left_to_merge(&log, "br1", "main").unwrap(), ["e1", "e3"]);
    log.merge("br1", "main", "carol", "m1", false).unwrap();
    assert!(left_to_merge(&log, "br1", "main").unwrap().is_empty());
    log.drop_rec("main", "d2", "br1", &[], &["e2"]);
    assert_eq!(left_to_merge(&log, "br1", "main").unwrap(), ["e2"]);
    // A drop on main says nothing about merges into another branch.
    log.fork("other", "main", 0);
    log.drop_rec("main", "d3", "br1", &["e2"], &[]);
    assert_eq!(
        left_to_merge(&log, "br1", "other").unwrap(),
        ["e1", "e2", "e3"]
    );
}

/// Like `checkpoint`, a `drop` is no unit on any undo stack: undo passes over it.
#[test]
fn drop_is_no_undo_unit() {
    let mut log = three_edits();
    log.commit("main", "t1", "alice", vec![ins(0, 1)]);
    log.drop_rec("main", "d1", "br1", &["e2"], &[]);
    let stacks = log.stacks("main", "alice");
    let ids: Vec<&str> = stacks.undo.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(ids, ["t1"]);
    let u = log.undo("main", "alice", "u1").unwrap();
    assert_eq!(u.record.undoes, Some(vec!["t1".to_string()]));
    assert!(log.stacks("main", "alice").undo.is_empty());
    // The drop stands.
    assert_eq!(dropped(&log, "main"), set(&["e2"]));
}

/// A damaged `drop` record is an error, not skipped: `dropped_for` and a merge report it.
#[test]
fn damaged_drop_record_is_an_error() {
    let mut log = three_edits();
    let seq = log.drop_rec("main", "d1", "br1", &["e2"], &[]);
    assert!(dropped_for(&log, &log.lineage("main").unwrap()).is_ok());
    log.damage_from("main", seq);
    let err = dropped_for(&log, &log.lineage("main").unwrap()).unwrap_err();
    assert_eq!(err.code, "INVALID_FORMAT");
    let err = plan_merge(&log, "br1", "main", "carol", "m1", false).unwrap_err();
    assert_eq!(err.code, "INVALID_FORMAT");
    // Records of other kinds are not looked at as drops.
    let mut log = three_edits();
    log.commit("main", "t1", "alice", vec![ins(0, 1)]);
    assert!(dropped_for(&log, &log.lineage("main").unwrap()).is_ok());
}

/// A cherry-pick copy of a dropped origin on another source branch is judged by its root.
#[test]
fn cherry_pick_copy_of_dropped_origin_is_excluded() {
    let mut log = three_edits();
    log.fork("other", "main", 0);
    log.pick("e2", "other", "dave", "k1")
        .unwrap()
        .expect("picked");
    log.drop_rec("main", "d1", "br1", &["e2"], &[]);
    let plan = log.merge("other", "main", "carol", "m1", false).unwrap();
    assert_eq!(excluded(&plan), [("k1", reasons::DROPPED)]);
    assert!(plan.merged.is_empty());
}

/// What remains after a drop stays listed when it depends on the dropped record: the merge
/// cannot take it without `--skip-conflicting`, so the branch is not merged.
#[test]
fn left_to_merge_keeps_dependents_of_dropped() {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "alice", vec![ins(30, 2)]);
    log.commit("br1", "e2", "alice", vec![trim("c4", Edge::End, secs(-1))]);
    log.drop_rec("main", "d1", "br1", &["e1"], &[]);
    assert_eq!(left_to_merge(&log, "br1", "main").unwrap(), ["e2"]);
    let err = log.merge("br1", "main", "carol", "m1", false).unwrap_err();
    assert_eq!(err.code, "CONFLICT");
}
