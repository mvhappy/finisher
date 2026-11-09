//! Rebased positions carried by the item edge they were written against (spec §4.3, as built
//! 2026-11-09; `rebase::anchors`): how an anchor is read in the op's own frame, how it resolves
//! at the head, its fallbacks (the other neighbour, then shifts), and what it changes on the
//! history paths (undo, merge), where the keyed oracle does not reach.

mod common;

use common::memlog::MemLog;
use common::moves::{move_to, with_v2};
use common::*;
use fin_core::rebase::anchors::{Anchor, anchors};
use fin_core::{
    ApplyMode, DeleteMode, Edge, InsertMode, ItemBody, ItemDelete, ItemInsert, ItemTrim, MemIds,
    Op, Project, Rational, TrimMode, rebase_tx,
};

/// `c1` [0,10), `c2` [10,20), `c3` [20,30) on `t1`; `t3` (V2) empty.
fn three() -> Project {
    with_v2(with_clips(&[10, 10, 10]))
}

fn ins(id: &str, track: &str, at: i64, len: i64) -> Op {
    insert(id, track, secs(at), "a1", Rational::zero(), secs(len))
}

fn overwrite(id: &str, at: Rational, duration: Rational) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t1".into(),
        at,
        duration,
        mode: InsertMode::Overwrite,
        body: ItemBody::Clip {
            asset: "a1".into(),
            source_in: Rational::zero(),
            speed: secs(1),
        },
    }
    .into()
}

fn lift(item: &str) -> Op {
    ItemDelete {
        item: item.into(),
        mode: DeleteMode::Lift,
    }
    .into()
}

fn overwrite_trim(item: &str, edge: Edge, delta: Rational) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode: TrimMode::Overwrite,
        alone: false,
    }
    .into()
}

fn anchor(start: Option<&str>, end: Option<&str>) -> Option<Anchor> {
    Some(Anchor {
        start: start.map(str::to_string),
        end: end.map(str::to_string),
        empty: None,
    })
}

/// Commits `ops` at `p` as `tx-a` (rev 1): the head, and the record.
fn commit(p: &Project, ops: Vec<Op>) -> (Project, fin_core::CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = "tx-a".into();
    let a = fin_core::apply_tx(p, &t, &mut MemIds::after(p)).unwrap();
    let c = fin_core::CommittedTx {
        rev: 1,
        tx: t,
        diff: a.diff,
        bindings: a.bindings,
        link: None,
    };
    (a.project, c)
}

fn stale(
    p: &Project,
    head: &Project,
    c: &fin_core::CommittedTx,
    ops: Vec<Op>,
) -> fin_core::rebase::Rebased {
    let mut t = tx(ops);
    t.tx_id = "tx-b".into();
    rebase_tx(
        p,
        head,
        &t,
        std::slice::from_ref(c),
        &mut MemIds::after(head),
    )
    .unwrap()
}

fn at_of(op: &Op) -> Rational {
    match op {
        Op::ItemInsert(i) => i.at,
        Op::ItemMove(m) => m.at,
        _ => panic!("{op:?}"),
    }
}

#[test]
fn anchors_are_read_in_the_op_frame() {
    let p = three();
    let t = tx(vec![
        // A cut: the item that follows (start) and the one before (end).
        ins("$x", "t1", 10, 2),
        // The track end; after `$x` the cut 12 is `$x`'s end and `c2`'s start.
        ins("$y", "t1", 32, 1),
        ins("$z", "t1", 12, 1),
        // Inside an item, and an empty track.
        overwrite("$o", secs(5), secs(10)),
        ins("$e", "t3", 7, 1),
    ]);
    let got = anchors(&p, &t, ApplyMode::Client).unwrap();
    assert_eq!(got[0], vec![anchor(Some("c2"), Some("c1"))]);
    assert_eq!(got[1], vec![anchor(None, Some("c3"))]);
    assert_eq!(got[2], vec![anchor(Some("c2"), Some("$x"))]);
    assert_eq!(got[3], vec![None]);
    assert_eq!(
        got[4],
        vec![Some(Anchor {
            start: None,
            end: None,
            empty: Some(secs(7)),
        })]
    );
}

#[test]
fn ops_without_positions_read_no_frame() {
    let t = tx(vec![delete("c1")]);
    assert!(anchors(&three(), &t, ApplyMode::Client).unwrap().is_empty());
}

/// A lift restore refills the span it was taken from: it takes no anchor, so a re-edged
/// neighbour does not move it (plan D1, rule 5).
#[test]
fn lift_restore_takes_no_anchor() {
    let p = three();
    let lifted = apply(&p, vec![lift("c2")]);
    let inverse = tx(lifted.inverse.clone());
    let got = anchors(&lifted.project, &inverse, ApplyMode::Internal).unwrap();
    assert!(
        matches!(&inverse.ops[0], Op::ItemRestore(_)),
        "{:?}",
        inverse.ops
    );
    assert_eq!(got[0], vec![None]);
}

/// The head lifts `c2` (no shift) and overwrite-trims `c1`'s end into the gap. Ours inserts
/// before `c2`, written at 10: `c2` is gone, so the other neighbour decides: `c1`'s end, now 12.
/// Through shifts it would stay at 10, inside `c1`.
#[test]
fn removed_start_anchor_falls_back_to_the_end_anchor() {
    let p = three();
    let (head, c) = commit(
        &p,
        vec![lift("c2"), overwrite_trim("c1", Edge::End, secs(2))],
    );
    let rb = stale(&p, &head, &c, vec![ins("$x", "t1", 10, 1)]);
    assert_eq!(at_of(&rb.tx.ops[0]), secs(12));
}

/// Both neighbours lifted: no anchor is left, so the position maps through shifts (a lift
/// records none) and stays where it was written.
#[test]
fn both_neighbours_gone_map_through_shifts() {
    let p = three();
    let (head, c) = commit(&p, vec![lift("c1"), lift("c2")]);
    let rb = stale(&p, &head, &c, vec![ins("$x", "t1", 10, 1)]);
    assert_eq!(at_of(&rb.tx.ops[0]), secs(10));
}

/// A position inside an item has no anchor: inside a range the head removed, it is `REMOVED`.
#[test]
fn unanchored_position_in_a_removed_range_is_removed() {
    let p = three();
    let (head, c) = commit(&p, vec![delete("c2")]);
    let mut t = tx(vec![overwrite("$o", secs(15), secs(10))]);
    t.tx_id = "tx-b".into();
    let e = rebase_tx(&p, &head, &t, &[c], &mut MemIds::after(&head)).unwrap_err();
    let all = &e.details["conflicts"];
    assert_eq!(all[0]["reason"], "REMOVED", "{e:?}");
}

/// The head ripple-moves `c2` to the empty `t3`. Ours inserts before `c2` on `t1`: the anchor
/// is on another track, so the op keeps its own track and the other neighbour decides (`c1`'s
/// end); the item is never moved to `t3` with it (plan D4).
#[test]
fn anchor_moved_to_another_track_keeps_the_op_track() {
    let p = three();
    let (head, c) = commit(
        &p,
        vec![move_to("c2", "t3", Rational::zero(), InsertMode::Ripple)],
    );
    let rb = stale(&p, &head, &c, vec![ins("$x", "t1", 10, 1)]);
    let Op::ItemInsert(i) = &rb.tx.ops[0] else {
        panic!()
    };
    assert_eq!((i.track.as_str(), i.at), ("t1", secs(10)));
}

/// Ours inserts after `c3` (the track end, 30); the head slides `c3` back two seconds, which
/// moves the end without a shift. The insert lands at `c3`'s end at the head, 28.
#[test]
fn end_anchor_follows_a_shiftless_slide() {
    let p = three();
    let (head, c) = commit(&p, vec![common::moves::slide("c3", secs(-2))]);
    let rb = stale(&p, &head, &c, vec![ins("$x", "t1", 30, 1)]);
    assert_eq!(at_of(&rb.tx.ops[0]), secs(28));
}

/// Undo of a ripple delete after another author ripple-moved the item that followed it
/// (plan D9): the restore goes back before that item, where it is now.
#[test]
fn undo_of_ripple_delete_goes_back_before_its_moved_follower() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10]));
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    // c1 c3 c4 → bob moves c3 to the end: c1 c4 c3.
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c3", "t1", secs(30), InsertMode::Ripple)],
    );
    log.undo("main", "alice", "u1").unwrap();
    let order: Vec<String> = starts(&log.state("main"), 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(order, ["c1", "c4", "c2", "c3"]);
    assert_well_formed(&log.state("main"));
}

/// Undo of a ripple move, after another author moved the item that followed its old place.
#[test]
fn undo_of_ripple_move_goes_back_before_its_moved_follower() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10]));
    // c1 c2 c3 c4 → alice moves c2 to the end: c1 c3 c4 c2.
    log.commit(
        "main",
        "e1",
        "alice",
        vec![move_to("c2", "t1", secs(40), InsertMode::Ripple)],
    );
    // bob moves c3 to the end: c1 c4 c2 c3.
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c3", "t1", secs(40), InsertMode::Ripple)],
    );
    log.undo("main", "alice", "u1").unwrap();
    let order: Vec<String> = starts(&log.state("main"), 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(order, ["c1", "c4", "c2", "c3"]);
    assert_well_formed(&log.state("main"));
}

/// A merge carries an insert written before `c3` to where `c3` is on the target, after the
/// target ripple-moved `c3` to its end.
#[test]
fn merged_insert_follows_its_moved_anchor() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10]));
    log.fork("br", "main", 0);
    log.commit("br", "e1", "alice", vec![ins("$x", "t1", 20, 1)]);
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c3", "t1", secs(40), InsertMode::Ripple)],
    );
    let plan = log.merge("br", "main", "alice", "m1", false).unwrap();
    assert!(plan.conflicts.is_empty(), "{:?}", plan.conflicts);
    let order: Vec<String> = starts(&log.state("main"), 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(order[..3], ["c1", "c2", "c4"]);
    assert_eq!(order[4], "c3");
    assert_well_formed(&log.state("main"));
}
