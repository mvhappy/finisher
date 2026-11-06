//! Undo and redo derived from the log (spec §4.5, plan 4B Task 12): the per-author stacks
//! simulated from a branch's own records, and the planners that rebase a unit's stored
//! inverse (or an undo's) over the records since, mirrors included.

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::history::{UnitEntry, UnitSource};
use fin_core::{CoreError, Edge, Link, Op, Project, Rational};

/// `c1` [0,10), `c2` [10,20), `c3` [20,30) on `t1`, asset `a1` (100 s).
fn genesis() -> Project {
    with_clips(&[10, 10, 10])
}

fn log() -> MemLog {
    MemLog::new(genesis())
}

/// A 1 s clip of `a1` at the end of `t1`.
fn append_clip(log: &MemLog, b: &str) -> Op {
    let p = log.state(b);
    let end = p.timelines[0].tracks[0]
        .items
        .last()
        .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap());
    insert("$x", "t1", end, "a1", Rational::zero(), secs(1))
}

fn edit(log: &mut MemLog, b: &str, tx: &str, author: &str) {
    let op = append_clip(log, b);
    log.commit(b, tx, author, vec![op]);
}

fn ids(stack: &[UnitEntry]) -> Vec<&str> {
    stack.iter().map(|u| u.id.as_str()).collect()
}

fn code(r: Result<fin_core::history::LogRec, CoreError>) -> CoreError {
    r.expect_err("expected an error")
}

#[derive(Debug, Clone, Copy)]
enum Act {
    Edit(&'static str, &'static str),
    Undo(&'static str, &'static str),
    Redo(&'static str, &'static str),
    Checkpoint(&'static str, &'static str),
}

#[test]
fn stack_simulation_table() {
    use Act::*;
    let table: &[(Act, &[&str], &[&str])] = &[
        (Edit("e1", "alice"), &["e1"], &[]),
        (Edit("b1", "bob"), &["e1"], &[]),
        (Edit("e2", "alice"), &["e1", "e2"], &[]),
        (Checkpoint("k1", "alice"), &["e1", "e2"], &[]),
        (Undo("u1", "alice"), &["e1"], &["u1"]),
        (Edit("b2", "bob"), &["e1"], &["u1"]),
        (Undo("u2", "alice"), &[], &["u1", "u2"]),
        // r1 redoes u2 (which undid e1) and is a unit of its own.
        (Redo("r1", "alice"), &["r1"], &["u1"]),
        (Undo("u3", "alice"), &[], &["u1", "u3"]),
        (Redo("r2", "alice"), &["r2"], &["u1"]),
        // A new edit clears the redo stack.
        (Edit("e3", "alice"), &["r2", "e3"], &[]),
    ];
    let mut log = log();
    for (i, (act, undo, redo)) in table.iter().enumerate() {
        match *act {
            Edit(tx, a) => edit(&mut log, "main", tx, a),
            Undo(tx, a) => drop(log.undo("main", a, tx).unwrap()),
            Redo(tx, a) => drop(log.redo("main", a, tx).unwrap()),
            Checkpoint(tx, a) => drop(log.append_link(
                "main",
                tx,
                a,
                Link::Checkpoint {
                    name: "k".into(),
                    recovers: None,
                },
                vec![],
            )),
        }
        let s = log.stacks("main", "alice");
        assert_eq!(
            (ids(&s.undo), ids(&s.redo)),
            (undo.to_vec(), redo.to_vec()),
            "row {i}: {act:?}"
        );
    }
    let bob = log.stacks("main", "bob");
    assert_eq!((ids(&bob.undo), ids(&bob.redo)), (vec!["b1", "b2"], vec![]));
    // The undo of r2's predecessor named the redo record as its unit.
    let u3 = fin_core::history::LogView::find(&log, "u3")
        .unwrap()
        .unwrap();
    assert_eq!(u3.record.undoes, Some(vec!["r1".to_string()]));
    // Every alice clip is back, bob's two are there: e1, b1, e2, b2, e3 in commit order.
    let clips: Vec<String> = starts(&log.state("main"), 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(clips.len(), 3 + 4, "{clips:?}");
}

#[test]
fn new_edit_clears_redo() {
    let mut log = log();
    edit(&mut log, "main", "e1", "alice");
    edit(&mut log, "main", "e2", "alice");
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(ids(&log.stacks("main", "alice").redo), ["u1"]);
    // Another author's edit leaves alice's redo stack alone; her own clears it.
    edit(&mut log, "main", "b1", "bob");
    assert_eq!(ids(&log.stacks("main", "alice").redo), ["u1"]);
    edit(&mut log, "main", "e3", "alice");
    let s = log.stacks("main", "alice");
    assert_eq!((ids(&s.undo), ids(&s.redo)), (vec!["e1", "e3"], vec![]));
    let err = code(log.redo("main", "alice", "r1"));
    assert_eq!(err.code, "NOTHING_TO_REDO");
    assert_eq!(err.details["author"], "alice");
}

#[test]
fn merge_group_is_one_unit() {
    let mut log = log();
    log.fork("br1", "main", 0);
    for (mid, n) in [("m1", 2), ("m2", 1)] {
        for k in 1..=n {
            let op = append_clip(&log, "main");
            let link = Link::Merge {
                merge_id: mid.into(),
                from: "br1".into(),
                origin: format!("x-{mid}-{k}"),
            };
            log.append_link("main", &format!("{mid}-{k}"), "alice", link, vec![op]);
        }
    }
    let s = log.stacks("main", "alice");
    assert_eq!(
        s.undo,
        vec![
            UnitEntry {
                id: "m1".into(),
                records: vec!["m1-1".into(), "m1-2".into()],
                last_seq: 2,
                source: UnitSource::Records,
            },
            UnitEntry {
                id: "m2".into(),
                records: vec!["m2-1".into()],
                last_seq: 3,
                source: UnitSource::Records,
            },
        ]
    );
    let after_m1 = fin_core::history::LogView::state_at(&log, "main", 2).unwrap();
    let u = log.undo("main", "alice", "u1").unwrap();
    assert_eq!(u.record.undoes, Some(vec!["m2".to_string()]));
    assert_eq!(log.state("main"), after_m1);
    // One undo takes the whole group, newest record first.
    let u = log.undo("main", "alice", "u2").unwrap();
    assert_eq!(u.record.undoes, Some(vec!["m1".to_string()]));
    assert_eq!(u.record.intent.ops.len(), 2);
    assert_eq!(log.state("main"), genesis());
}

#[test]
fn merged_away_drops_stack_only_for_merge_into_non_descendant() {
    let mut log = log();
    log.fork("br1", "main", 0);
    for tx in ["e1", "e2", "e3"] {
        edit(&mut log, "br1", tx, "alice");
    }
    let copy = |log: &mut MemLog, b: &str, tx: &str, link: Link| {
        let op = append_clip(log, b);
        log.append_link(b, tx, "alice", link, vec![op]);
    };
    // A merge into a child of br1 and a cherry-pick into main drop nothing.
    log.fork("br2", "br1", 3);
    let into_child = Link::Merge {
        merge_id: "mc".into(),
        from: "br1".into(),
        origin: "e3".into(),
    };
    copy(&mut log, "br2", "mc-1", into_child);
    let pick = Link::CherryPick {
        origin: "e3".into(),
        from: "br1".into(),
        origin_hash: None,
        recovers: None,
    };
    copy(&mut log, "main", "cp1", pick);
    assert_eq!(ids(&log.stacks("br1", "alice").undo), ["e1", "e2", "e3"]);

    // A merge of e2 into main (not a descendant) drops e2 and everything below it.
    let into_main = Link::Merge {
        merge_id: "mm".into(),
        from: "br1".into(),
        origin: "e2".into(),
    };
    copy(&mut log, "main", "mm-1", into_main);
    assert_eq!(ids(&log.stacks("br1", "alice").undo), ["e3"]);
    // The merging author's copies are units on the target.
    assert_eq!(ids(&log.stacks("main", "alice").undo), ["cp1", "mm"]);
}

#[test]
fn redo_is_a_unit_with_redo_source() {
    let mut log = log();
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    let after_e1 = log.state("main");
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(log.state("main"), genesis());
    let r1 = log.redo("main", "alice", "r1").unwrap();
    assert_eq!(r1.record.redoes.as_deref(), Some("u1"));
    assert_eq!(log.state("main"), after_e1);
    assert_eq!(
        log.stacks("main", "alice").undo,
        vec![UnitEntry {
            id: "r1".into(),
            records: vec!["r1".into()],
            last_seq: 3,
            source: UnitSource::Redo { tx_id: "r1".into() },
        }]
    );
    // The next undo takes the redo record's own inverse and names it.
    let u2 = log.undo("main", "alice", "u2").unwrap();
    assert_eq!(u2.record.undoes, Some(vec!["r1".to_string()]));
    assert_eq!(u2.record.intent.ops, r1.record.inverse);
    assert_eq!(log.state("main"), genesis());
    // And the redo of that undo recreates the state again.
    log.redo("main", "alice", "r2").unwrap();
    assert_eq!(log.state("main"), after_e1);
}

#[test]
fn undo_maps_over_later_insert_before_it() {
    let mut log = log();
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    // Bob inserts 5 s at 0: everything after moves by 5.
    let w = insert("$w", "t1", secs(0), "a1", secs(0), secs(5));
    log.commit("main", "b1", "bob", vec![w]);
    let u = log.undo("main", "alice", "u1").unwrap();
    let Op::ItemRestore(r) = &u.record.intent.ops[0] else {
        panic!("{:?}", u.record.intent.ops)
    };
    assert_eq!((r.item.id.as_str(), r.item.start), ("c2", secs(15)));
    assert_eq!(
        starts(&log.state("main"), 0),
        [
            ("c4".to_string(), secs(0)),
            ("c1".to_string(), secs(5)),
            ("c2".to_string(), secs(15)),
            ("c3".to_string(), secs(25)),
        ]
    );
    assert_well_formed(&log.state("main"));
}

#[test]
fn undo_of_undone_pair_is_exact_via_mirrors() {
    let mut log = log();
    // X Y Z; alice deletes Y; bob deletes Z and undoes that. Restoring Y must put it before Z
    // again: Z's delete and its undo are a mirrored pair, so Y's position (Z's start edge)
    // comes back through it instead of being pushed after the restored Z.
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    log.commit("main", "b1", "bob", vec![delete("c3")]);
    log.undo("main", "bob", "bu").unwrap();
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(log.state("main"), genesis());
}

#[test]
fn undo_redo_undo_again_is_exact() {
    let mut log = log();
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    let after_e1 = log.state("main");
    log.undo("main", "alice", "u1").unwrap();
    log.redo("main", "alice", "r1").unwrap();
    let u2 = log.undo("main", "alice", "u2").unwrap();
    assert_eq!(u2.record.undoes, Some(vec!["r1".to_string()]));
    assert_eq!(log.state("main"), genesis());
    log.redo("main", "alice", "r2").unwrap();
    assert_eq!(log.state("main"), after_e1);
}

#[test]
fn undo_conflicts_when_another_author_removed_the_clip() {
    let mut log = log();
    let trim = trim("c2", Edge::End, secs(-2));
    log.commit("main", "e1", "alice", vec![trim]);
    log.commit("main", "b1", "bob", vec![delete("c2")]);
    let before = log.head_seq("main");
    let err = code(log.undo("main", "alice", "u1"));
    assert_eq!(err.code, "CONFLICT");
    let c = &err.details["conflicts"][0];
    assert_eq!(
        (c["reason"].as_str(), c["againstTx"].as_str()),
        (Some("REMOVED"), Some("b1"))
    );
    assert_eq!(log.head_seq("main"), before, "nothing appended");
}

#[test]
fn nothing_to_undo_below_the_fork_or_for_another_author() {
    let mut log = log();
    edit(&mut log, "main", "e1", "alice");
    log.fork("br1", "main", 1);
    // e1 is main's, not br1's own.
    let err = code(log.undo("br1", "alice", "u1"));
    assert_eq!(err.code, "NOTHING_TO_UNDO");
    assert_eq!(err.details["branch"], "br1");
    assert_eq!(code(log.undo("main", "bob", "u1")).code, "NOTHING_TO_UNDO");
    assert_eq!(
        code(log.redo("main", "alice", "r1")).code,
        "NOTHING_TO_REDO"
    );
    // On br1, its own edits only.
    edit(&mut log, "br1", "f1", "alice");
    let u = log.undo("br1", "alice", "u1").unwrap();
    assert_eq!(u.record.undoes, Some(vec!["f1".to_string()]));
    assert_eq!(code(log.undo("br1", "alice", "u2")).code, "NOTHING_TO_UNDO");
}

#[test]
fn stale_edit_over_an_undo_pair_is_mirrored() {
    // A stale commit whose interval holds an undo record and its unit: the pair is mirrored
    // (`steps_from` from the undo record's link), so an insert at Z's start lands before Z,
    // as the same insert at the head does.
    let mut log = log();
    log.commit("main", "e1", "alice", vec![delete("c3")]);
    log.undo("main", "alice", "u1").unwrap();
    let w = insert("$w", "t1", secs(20), "a1", secs(0), secs(5));
    log.commit_at("main", "b1", "bob", 0, vec![w]).unwrap();
    assert_eq!(
        starts(&log.state("main"), 0),
        [
            ("c1".to_string(), secs(0)),
            ("c2".to_string(), secs(10)),
            ("c4".to_string(), secs(20)),
            ("c3".to_string(), secs(25)),
        ]
    );
}

#[test]
fn merged_away_drops_the_redo_stack_too() {
    // On br1: e1, e2, undo e2; all three merged into main. Both of br1's stacks are dropped
    // (arch §6.4), so the redo of e2 is gone on br1 as well.
    let mut log = log();
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "alice", vec![delete("c1")]);
    log.commit("br1", "e2", "alice", vec![delete("c2")]);
    log.undo("br1", "alice", "u1").unwrap();
    let s = log.stacks("br1", "alice");
    assert_eq!((ids(&s.undo), ids(&s.redo)), (vec!["e1"], vec!["u1"]));
    let plan = log.merge("br1", "main", "carol", "m", false).unwrap();
    assert_eq!(plan.merged, ["e1", "e2", "u1"]);
    assert_eq!(log.state("main"), log.state("br1"));
    let s = log.stacks("br1", "alice");
    assert!(s.undo.is_empty() && s.redo.is_empty(), "{s:?}");
    assert_eq!(code(log.redo("br1", "alice", "r1")).code, "NOTHING_TO_REDO");
}

// `undo --to <checkpoint>` (plan 4B Task 15).

fn rec(log: &MemLog, tx: &str) -> fin_core::history::LogRec {
    fin_core::history::LogView::find(log, tx).unwrap().unwrap()
}

fn undoes(r: &fin_core::history::LogRec) -> Vec<&str> {
    r.record
        .undoes
        .iter()
        .flatten()
        .map(String::as_str)
        .collect()
}

#[test]
fn undo_to_checkpoint_one_record_newest_first() {
    let mut log = log();
    log.commit("main", "e0", "alice", vec![delete("c1")]);
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    let at_cp = log.state("main");
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    edit(&mut log, "main", "e2", "alice");
    let head = log.head_seq("main");
    let u = log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(u.seq, head + 1, "one record");
    assert_eq!(undoes(&u), ["e2", "e1"]);
    // The units' stored inverses, newest unit first, one op per op (nothing moved them).
    let mut want = rec(&log, "e2").record.inverse;
    want.extend(rec(&log, "e1").record.inverse);
    assert_eq!(u.record.intent.ops, want);
    assert_eq!(log.state("main"), at_cp);
    let s = log.stacks("main", "alice");
    assert_eq!((ids(&s.undo), ids(&s.redo)), (vec!["e0"], vec!["ut"]));
}

/// Alice and Bob interleaved on `log`: a checkpoint, then alice's two deletes and an append
/// around bob's insert at 0. Returns the checkpoint's seq.
fn interleaved(log: &mut MemLog) -> u64 {
    log.commit("main", "e0", "alice", vec![delete("c3")]);
    let cp = log.checkpoint("main", "k1", "bob", "v1");
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    let w = insert("$w", "t1", secs(0), "a1", secs(0), secs(5));
    log.commit("main", "b1", "bob", vec![w]);
    log.commit("main", "e2", "alice", vec![delete("c1")]);
    edit(log, "main", "e3", "alice");
    cp
}

#[test]
fn undo_to_checkpoint_skips_other_authors_and_rebases_over_them() {
    let mut log = log();
    let cp = interleaved(&mut log);
    let u = log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(undoes(&u), ["e3", "e2", "e1"]);
    // Bob's clip stays at 0; alice's are back where they were, after it.
    assert_eq!(
        starts(&log.state("main"), 0),
        [
            ("c4".to_string(), secs(0)),
            ("c1".to_string(), secs(5)),
            ("c2".to_string(), secs(15)),
        ]
    );
    assert_well_formed(&log.state("main"));
    assert_eq!(ids(&log.stacks("main", "bob").undo), ["b1"]);
    assert_eq!(ids(&log.stacks("main", "alice").undo), ["e0"]);
}

#[test]
fn undo_to_is_the_undos_in_a_row() {
    let (mut one, mut many) = (log(), log());
    let cp = interleaved(&mut one);
    interleaved(&mut many);
    let u = one.undo_to("main", "alice", cp, "ut").unwrap();
    let mut ops = vec![];
    for k in 1..=3 {
        ops.extend(
            many.undo("main", "alice", &format!("u{k}"))
                .unwrap()
                .record
                .intent
                .ops,
        );
    }
    assert_eq!(u.record.intent.ops, ops);
    assert_eq!(one.state("main"), many.state("main"));
}

#[test]
fn undo_to_includes_redo_units() {
    let mut log = log();
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    log.commit("main", "e1", "alice", vec![delete("c1")]);
    log.commit("main", "e2", "alice", vec![delete("c2")]);
    log.undo("main", "alice", "u1").unwrap();
    log.redo("main", "alice", "r1").unwrap();
    let u = log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(undoes(&u), ["r1", "e1"]);
    assert_eq!(log.state("main"), genesis());
}

#[test]
fn redo_of_a_unit_from_before_the_checkpoint_is_kept() {
    // e1 is part of the checkpoint's state; undone and redone after it, the redo unit stands
    // for e1, so `undo --to` stops there and the state is the checkpoint's again.
    let mut log = log();
    log.commit("main", "e1", "alice", vec![delete("c1")]);
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    let at_cp = log.state("main");
    log.undo("main", "alice", "u1").unwrap();
    log.redo("main", "alice", "r1").unwrap();
    log.commit("main", "e2", "alice", vec![delete("c2")]);
    let u = log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(undoes(&u), ["e2"]);
    assert_eq!(log.state("main"), at_cp);
    let err = code(log.undo_to("main", "alice", cp, "ut2"));
    assert_eq!(err.code, "NOTHING_TO_UNDO");
    assert_eq!(err.details["checkpointSeq"], cp);
}

#[test]
fn redo_chain_stands_for_the_first_unit() {
    // e1 before the checkpoint, undone and redone twice after it: `r2` stands for e1 through
    // `r1`, so it is kept; e2 after it goes. The same chain on e2 counts as after.
    let mut log = log();
    log.commit("main", "e1", "alice", vec![delete("c1")]);
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    let at_cp = log.state("main");
    log.undo("main", "alice", "u1").unwrap();
    log.redo("main", "alice", "r1").unwrap();
    log.undo("main", "alice", "u2").unwrap();
    log.redo("main", "alice", "r2").unwrap();
    log.commit("main", "e2", "alice", vec![delete("c2")]);
    log.undo("main", "alice", "u3").unwrap();
    log.redo("main", "alice", "r3").unwrap();
    log.undo("main", "alice", "u4").unwrap();
    log.redo("main", "alice", "r4").unwrap();
    let u = log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(undoes(&u), ["r4"]);
    assert_eq!(log.state("main"), at_cp);
}

#[test]
fn redo_reverses_whole_undo_to() {
    let mut log = log();
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    log.commit("main", "e1", "alice", vec![delete("c1")]);
    edit(&mut log, "main", "e2", "alice");
    let at_end = log.state("main");
    log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(log.state("main"), genesis());
    let r = log.redo("main", "alice", "r1").unwrap();
    assert_eq!(r.record.redoes.as_deref(), Some("ut"));
    assert_eq!(log.state("main"), at_end);
    // The redo is one unit: one undo takes it all back again.
    let u = log.undo("main", "alice", "u2").unwrap();
    assert_eq!(undoes(&u), ["r1"]);
    assert_eq!(log.state("main"), genesis());
}

#[test]
fn undo_to_conflict_is_all_or_nothing() {
    let mut log = log();
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    let trim = |c: &str| trim(c, Edge::End, secs(-2));
    log.commit("main", "e1", "alice", vec![trim("c2")]);
    log.commit("main", "e2", "alice", vec![trim("c3")]);
    edit(&mut log, "main", "e3", "alice");
    log.commit("main", "b1", "bob", vec![delete("c2"), delete("c3")]);
    let before = log.head_seq("main");
    let err = code(log.undo_to("main", "alice", cp, "ut"));
    assert_eq!(err.code, "CONFLICT");
    let all = err.details["conflicts"].as_array().unwrap();
    let got: Vec<_> = all
        .iter()
        .map(|c| {
            (
                c["txId"].as_str().unwrap(),
                c["opIndex"].as_u64().unwrap(),
                c["reason"].as_str().unwrap(),
                c["againstTx"].as_str(),
                c["details"]["unit"].as_str().unwrap(),
            )
        })
        .collect();
    // e3's inverse (op 0) carries; e2's (op 1) and e1's (op 2) meet bob's deletes.
    assert_eq!(
        got,
        [
            ("ut", 1, "REMOVED", Some("b1"), "e2"),
            ("ut", 2, "REMOVED", Some("b1"), "e1"),
        ]
    );
    assert_eq!(log.head_seq("main"), before, "nothing appended");
}

#[test]
fn undo_to_before_fork_undoes_all_own() {
    let mut log = log();
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    log.commit("main", "e1", "alice", vec![delete("c3")]);
    log.fork("br1", "main", 2);
    let at_fork = log.state("br1");
    log.commit("br1", "f1", "alice", vec![delete("c1")]);
    edit(&mut log, "br1", "g1", "bob");
    log.commit("br1", "f2", "alice", vec![delete("c2")]);
    let u = log.undo_to("br1", "alice", cp, "ut").unwrap();
    assert_eq!(undoes(&u), ["f2", "f1"]);
    // main's e1 is below the fork: it stays; bob's clip too.
    let clips: Vec<String> = starts(&log.state("br1"), 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(clips, ["c1", "c2", "c4"]);
    assert_eq!(starts(&at_fork, 0).len(), 2);
    // A checkpoint exactly at the fork takes them all too.
    let err = code(log.undo_to("br1", "alice", 2, "ut2"));
    assert_eq!(err.code, "NOTHING_TO_UNDO");
}

#[test]
fn undo_to_takes_a_merge_unit_whole() {
    let mut log = log();
    log.fork("br1", "main", 0);
    log.commit("br1", "f1", "bob", vec![delete("c1")]);
    log.commit("br1", "f2", "bob", vec![delete("c2")]);
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    log.commit("main", "e1", "alice", vec![delete("c3")]);
    log.merge("br1", "main", "alice", "m", false).unwrap();
    let u = log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(undoes(&u), ["m", "e1"]);
    assert_eq!(u.record.intent.ops.len(), 3);
    assert_eq!(log.state("main"), genesis());
}

#[test]
fn stale_edit_over_an_undo_to_is_mirrored() {
    // As `stale_edit_over_an_undo_pair_is_mirrored`, with one `undo --to` record for two
    // units: a stale commit's interval cuts it into one part per unit, each mirroring its
    // unit, so an insert at Z's start lands before Z.
    let mut log = log();
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    log.commit("main", "e1", "alice", vec![delete("c3")]);
    log.commit("main", "e2", "alice", vec![delete("c1")]);
    log.undo_to("main", "alice", cp, "ut").unwrap();
    let w = insert("$w", "t1", secs(20), "a1", secs(0), secs(5));
    log.commit_at("main", "b1", "bob", cp, vec![w]).unwrap();
    assert_eq!(
        starts(&log.state("main"), 0),
        [
            ("c1".to_string(), secs(0)),
            ("c2".to_string(), secs(10)),
            ("c4".to_string(), secs(20)),
            ("c3".to_string(), secs(25)),
        ]
    );
}

/// A checkpoint, e1 (deletes Z), its undo and redo, e2 (deletes X); returns the
/// checkpoint's seq and e1's.
fn redo_inside(log: &mut MemLog) -> (u64, u64) {
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    let e1 = log.commit("main", "e1", "alice", vec![delete("c3")]).seq;
    log.undo("main", "alice", "u1").unwrap();
    log.redo("main", "alice", "r1").unwrap();
    log.commit("main", "e2", "alice", vec![delete("c1")]);
    (cp, e1)
}

#[test]
fn stale_edit_over_an_undo_to_with_a_redo_unit_pairs_each_unit() {
    // The `undo --to` record names [e2, r1]. Seen from a stale base after e1, u1 stands
    // alone and r1 mirrors it, so r1 is paired already: cut per unit, the e2 part still
    // pairs with e2, and a stale edit over the record lands as over the undos in a row.
    let (mut one, mut many) = (log(), log());
    let (cp, e1) = redo_inside(&mut one);
    redo_inside(&mut many);
    let u = one.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(undoes(&u), ["e2", "r1"]);
    many.undo("main", "alice", "ua").unwrap();
    many.undo("main", "alice", "ub").unwrap();
    let since: Vec<fin_core::CommittedTx> =
        fin_core::history::LogView::records(&one, "main", e1, one.head_seq("main"))
            .unwrap()
            .iter()
            .map(|r| r.committed().unwrap())
            .collect();
    let steps = fin_core::rebase::steps_from(&since.iter().collect::<Vec<_>>()).unwrap();
    let at = |tx: &str| steps.iter().position(|s| s.tx_id == tx).unwrap();
    let parts: Vec<Option<usize>> = steps
        .iter()
        .filter(|s| s.tx_id == "ut")
        .map(|s| s.mirror_of)
        .collect();
    assert_eq!(parts.len(), 2, "one part per unit: {parts:?}");
    assert_eq!(parts[0], Some(at("e2")), "{parts:?}");
    for log in [&mut one, &mut many] {
        let w = insert("$w", "t1", secs(0), "a1", secs(0), secs(5));
        log.commit_at("main", "b1", "bob", e1, vec![w]).unwrap();
    }
    assert_eq!(one.state("main"), many.state("main"));
}

#[test]
fn redo_after_the_checkpoint_of_an_undo_before_it_is_taken() {
    // e1, its undo, the checkpoint, then the redo: the checkpoint's state lacks e1, so the
    // redo was made after it and `undo --to` takes it back.
    let mut log = log();
    log.commit("main", "e1", "alice", vec![delete("c1")]);
    log.undo("main", "alice", "u1").unwrap();
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    let at_cp = log.state("main");
    log.redo("main", "alice", "r1").unwrap();
    let u = log.undo_to("main", "alice", cp, "ut").unwrap();
    assert_eq!(undoes(&u), ["r1"]);
    assert_eq!(log.state("main"), at_cp);
}

#[test]
fn redo_straddling_the_checkpoint_is_refused() {
    // cp0; e1; cpB; e2; undo --to cp0 (both); redo: the redo stands for e1 (before cpB) and
    // e2 (after it). Neither undoing it nor keeping it gives cpB's state: refused whole.
    let mut log = log();
    let cp0 = log.checkpoint("main", "k0", "alice", "v0");
    log.commit("main", "e1", "alice", vec![delete("c1")]);
    let cpb = log.checkpoint("main", "kb", "alice", "vb");
    log.commit("main", "e2", "alice", vec![delete("c2")]);
    log.undo_to("main", "alice", cp0, "ut0").unwrap();
    log.redo("main", "alice", "r1").unwrap();
    edit(&mut log, "main", "e3", "alice");
    let head = log.head_seq("main");
    let err = code(log.undo_to("main", "alice", cpb, "utb"));
    assert_eq!(err.code, "CONFLICT");
    let c = &err.details["conflicts"][0];
    assert_eq!(
        (
            c["reason"].as_str(),
            c["againstTx"].as_str(),
            c["opIndex"].as_u64(),
            c["details"]["unit"].as_str(),
            c["details"]["checkpointSeq"].as_u64(),
        ),
        (
            Some("STRADDLES_CHECKPOINT"),
            Some("r1"),
            Some(1),
            Some("r1"),
            Some(cpb)
        ),
        "{err:?}"
    );
    assert_eq!(err.details["conflicts"].as_array().unwrap().len(), 1);
    assert!(c["fix"].as_str().unwrap().contains("plain undos"));
    assert_eq!(log.head_seq("main"), head, "nothing appended");
    // An earlier checkpoint takes the redo whole; plain undos work too.
    let u = log.undo_to("main", "alice", cp0, "ut1").unwrap();
    assert_eq!(undoes(&u), ["e3", "r1"]);
    assert_eq!(log.state("main"), genesis());
}

/// cp; e1 deletes Z; e2 deletes X; then `undo --to` cp, or two undos. Returns e1's seq.
fn two_units(log: &mut MemLog, to: bool) -> u64 {
    let cp = log.checkpoint("main", "k1", "alice", "v1");
    let e1 = log.commit("main", "e1", "alice", vec![delete("c3")]).seq;
    log.commit("main", "e2", "alice", vec![delete("c1")]);
    if to {
        log.undo_to("main", "alice", cp, "ut").unwrap();
    } else {
        log.undo("main", "alice", "ua").unwrap();
        log.undo("main", "alice", "ub").unwrap();
    }
    e1
}

#[test]
fn stale_edit_between_undo_to_units_is_the_undos_in_a_row() {
    // A stale base between e1 and e2: only e2 is in the interval. The record is cut into e2's
    // part (paired) and an unpaired tail, as the two undos would be.
    let trim = trim("c1", Edge::End, secs(-2));
    let w = |at: i64| insert("$w", "t1", secs(at), "a1", secs(0), secs(5));
    for (n, ops) in [vec![trim], vec![w(0)], vec![w(5)], vec![w(10)], vec![w(20)]]
        .into_iter()
        .enumerate()
    {
        let (mut one, mut many) = (log(), log());
        let e1 = two_units(&mut one, true);
        two_units(&mut many, false);
        let a = one
            .commit_at("main", "b1", "bob", e1, ops.clone())
            .map(|_| ());
        let b = many.commit_at("main", "b1", "bob", e1, ops).map(|_| ());
        assert_eq!(a.map_err(|e| e.code), b.map_err(|e| e.code), "case {n}");
        assert_eq!(one.state("main"), many.state("main"), "case {n}");
    }
}
