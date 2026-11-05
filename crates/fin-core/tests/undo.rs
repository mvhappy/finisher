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
    let end = p.timeline.tracks[0]
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

fn delete(clip: &str) -> Op {
    Op::ClipDelete { clip: clip.into() }
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
            Checkpoint(tx, a) => {
                drop(log.append_link("main", tx, a, Link::Checkpoint { name: "k".into() }, vec![]))
            }
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
    let Op::ClipRestore { clip, .. } = &u.record.intent.ops[0] else {
        panic!("{:?}", u.record.intent.ops)
    };
    assert_eq!((clip.id.as_str(), clip.start), ("c2", secs(15)));
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
    let trim = Op::ClipTrim {
        clip: "c2".into(),
        edge: Edge::End,
        delta: secs(-2),
    };
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
