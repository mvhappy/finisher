//! Placement of restored items on the history paths (owner decisions of 2026-11-09; SP4 §4.3
//! *Placement of restored items*; `rebase::placement`): an undone, redone or copied ripple
//! delete or move goes back right after its left neighbour, else before its right one; with
//! both gone a person's restore snaps to the nearest boundary (`relocated`) and an agent's is
//! `NEIGHBOURS_GONE`; every restored item reports its outcome.

mod common;

use common::memlog::MemLog;
use common::moves::move_to;
use common::*;
use fin_core::rebase::placement::{
    NeighbourChange as Change, Placement, PlacementOutcome as Outcome, WhenLost,
};
use fin_core::rebase::reasons;
use fin_core::{InsertMode, Op, Project, Rational, TrackRemove};

fn order(p: &Project) -> Vec<String> {
    starts(p, 0).into_iter().map(|(id, _)| id).collect()
}

fn one(placements: &[Placement]) -> &Placement {
    assert_eq!(placements.len(), 1, "{placements:?}");
    &placements[0]
}

/// Nothing concurrent: the item goes back where it was, and the outcome is `clean`.
#[test]
fn plain_undo_is_clean() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10]));
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(order(&log.state("main")), ["c1", "c2", "c3"]);
    let p = one(&log.placements);
    assert_eq!((p.item.as_str(), p.outcome), ("c2", Outcome::Clean));
    assert_eq!(p.tx_id, "u1");
    assert!(p.landed.is_none() && p.causes.is_empty(), "{p:?}");
}

/// Decision 1: the left neighbour moved: the item follows it (relocated), and the cause is
/// bob's move of `c1`.
#[test]
fn undo_follows_its_moved_left_neighbour() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10]));
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    // c1 c3 c4 → bob moves c1 to the end: c3 c4 c1.
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c1", "t1", secs(30), InsertMode::Ripple)],
    );
    log.undo("main", "alice", "u1").unwrap();
    let head = log.state("main");
    assert_eq!(order(&head), ["c3", "c4", "c1", "c2"]);
    let p = one(&log.placements);
    assert_eq!(p.outcome, Outcome::Relocated);
    let landed = p.landed.as_ref().unwrap();
    assert_eq!(landed.track, "t1");
    assert_eq!(landed.at, secs(30));
    assert_eq!(landed.frame, Some(900));
    assert_eq!(landed.after.as_deref(), Some("c1"));
    assert_eq!(landed.before, None);
    assert_eq!(p.causes.len(), 1, "{p:?}");
    let c = &p.causes[0];
    assert_eq!(
        (c.neighbour.as_str(), c.change, c.tx_id.as_str()),
        ("c1", Change::Moved, "b1")
    );
    assert_eq!(c.author.as_deref(), Some("bob"));
    assert_well_formed(&head);
}

/// Decision 1: the left neighbour is gone and the right one is there: before it (relocated,
/// because bob removed `c1`).
#[test]
fn undo_goes_before_its_right_neighbour_when_the_left_is_gone() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10]));
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    // c1 c3 c4 → bob moves c3 to the end, then deletes c1: c4 c3.
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c3", "t1", secs(30), InsertMode::Ripple)],
    );
    log.commit("main", "b2", "bob", vec![delete("c1")]);
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(order(&log.state("main")), ["c4", "c2", "c3"]);
    let p = one(&log.placements);
    assert_eq!(p.outcome, Outcome::Relocated);
    let landed = p.landed.as_ref().unwrap();
    assert_eq!(
        (landed.after.as_deref(), landed.before.as_deref()),
        (Some("c4"), Some("c3"))
    );
    let causes: Vec<_> = p
        .causes
        .iter()
        .map(|c| (c.neighbour.as_str(), c.change, c.tx_id.as_str()))
        .collect();
    assert_eq!(causes, [("c1", Change::Removed, "b2")]);
}

/// The item that preceded it is gone, the follower too: one person's undo goes back at the old
/// time, snapped to the nearest boundary (relocated), with both causes.
fn both_gone(when_lost: WhenLost) -> (MemLog, Result<(), fin_core::CoreError>) {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10, 10]));
    log.when_lost = when_lost;
    // c1 [0,10) c2 [10,20) c3 c4 c5 → alice deletes c2: c1 c3 c4 c5.
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    // bob ripple-moves c4 to the front (c4 c1 c3 c5), then deletes c1 and c3: c4 [0,10)
    // c5 [10,20). The old time, 10, maps through the shifts to 10.
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c4", "t1", secs(0), InsertMode::Ripple)],
    );
    log.commit("main", "b2", "bob", vec![delete("c1")]);
    log.commit("main", "b3", "carol", vec![delete("c3")]);
    let r = log.undo("main", "alice", "u1").map(|_| ());
    (log, r)
}

#[test]
fn both_neighbours_gone_snaps_for_a_person() {
    let (log, r) = both_gone(WhenLost::Snap);
    r.unwrap();
    let head = log.state("main");
    assert_eq!(order(&head), ["c4", "c2", "c5"]);
    assert_eq!(clip(&head, "c2").start, secs(10));
    let p = one(&log.placements);
    assert_eq!(p.outcome, Outcome::Relocated);
    let landed = p.landed.as_ref().unwrap();
    assert_eq!(
        (landed.after.as_deref(), landed.before.as_deref()),
        (Some("c4"), Some("c5"))
    );
    let causes: Vec<_> = p
        .causes
        .iter()
        .map(|c| {
            (
                c.neighbour.as_str(),
                c.change,
                c.tx_id.as_str(),
                c.author.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        causes,
        [
            ("c1", Change::Removed, "b2", Some("bob")),
            ("c3", Change::Removed, "b3", Some("carol")),
        ]
    );
    assert_well_formed(&head);
}

/// The old time is inside an item at the head: it snaps to that item's nearer edge.
#[test]
fn snap_takes_the_nearest_boundary() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10]));
    // c1 [0,10) c2 [10,20) c3 [20,30) → alice deletes c2: c1 c3 [10,20).
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    // bob lifts c1 and c3 (no shifts: the old time stays 10), then overwrites x [4,17).
    log.commit("main", "b1", "bob", vec![lift("c1"), lift("c3")]);
    let x: Op = fin_core::ItemInsert {
        id: "$x".into(),
        track: "t1".into(),
        at: secs(4),
        duration: secs(13),
        mode: InsertMode::Overwrite,
        body: fin_core::ItemBody::Clip {
            asset: "a1".into(),
            source_in: Rational::zero(),
            speed: secs(1),
        },
    }
    .into();
    log.commit("main", "b2", "bob", vec![x]);
    log.undo("main", "alice", "u1").unwrap();
    let head = log.state("main");
    // 10 is inside x [4,17): 4 is nearer than 17.
    assert_eq!(clip(&head, "c2").start, secs(4));
    let p = one(&log.placements);
    assert_eq!(p.outcome, Outcome::Relocated);
    let landed = p.landed.as_ref().unwrap();
    assert_eq!(landed.after, None);
    assert_well_formed(&head);
}

fn lift(item: &str) -> Op {
    fin_core::ItemDelete {
        item: item.into(),
        mode: fin_core::DeleteMode::Lift,
    }
    .into()
}

/// Decision 2: an agent's undo with both neighbours gone is a typed conflict, blamed on the
/// record that removed the left neighbour, and nothing is restored.
#[test]
fn both_neighbours_gone_is_a_conflict_for_an_agent() {
    let (log, r) = both_gone(WhenLost::Refuse);
    let e = r.unwrap_err();
    assert_eq!(e.code, "CONFLICT");
    assert_eq!(e.details["reason"], reasons::NEIGHBOURS_GONE);
    assert_eq!(e.details["againstTx"], "b2");
    let c = &e.details["conflicts"][0];
    assert_eq!(c["details"]["placement"]["outcome"], "conflict");
    assert_eq!(c["details"]["placement"]["item"], "c2");
    assert_eq!(
        c["details"]["placement"]["causes"][1]["neighbour"], "c3",
        "{c}"
    );
    assert!(e.fix.as_deref().unwrap().contains("decide"), "{e:?}");
    assert_eq!(order(&log.state("main")), ["c4", "c5"]);
}

/// Decision 2: a track that is gone is a conflict for a person too.
#[test]
fn track_gone_is_a_conflict_for_everyone() {
    for when_lost in [WhenLost::Snap, WhenLost::Refuse] {
        let mut log = MemLog::new(with_clips(&[10, 10]));
        log.when_lost = when_lost;
        log.commit("main", "e1", "alice", vec![delete("c2")]);
        let rm: Op = TrackRemove {
            track: "t1".into(),
            with_items: true,
        }
        .into();
        log.commit("main", "b1", "bob", vec![rm]);
        let e = log.undo("main", "alice", "u1").unwrap_err();
        assert_eq!(e.code, "CONFLICT", "{when_lost:?}: {e:?}");
        assert_ne!(e.details["reason"], reasons::NEIGHBOURS_GONE);
    }
}

/// Decision 2: an asset that is gone is a conflict for a person too.
#[test]
fn asset_gone_is_a_conflict_for_everyone() {
    for when_lost in [WhenLost::Snap, WhenLost::Refuse] {
        let a = apply(
            &with_clips(&[10, 10]),
            vec![
                av_asset("$b", secs(100)),
                insert("$x", "t1", secs(20), "$b", Rational::zero(), secs(5)),
            ],
        );
        let (b, x) = (a.bindings["$b"].clone(), a.bindings["$x"].clone());
        let mut log = MemLog::new(a.project);
        log.when_lost = when_lost;
        log.commit("main", "e1", "alice", vec![delete(&x)]);
        log.commit("main", "b1", "bob", vec![Op::AssetRemove { id: b }]);
        let e = log.undo("main", "alice", "u1").unwrap_err();
        assert_eq!(e.code, "CONFLICT", "{when_lost:?}: {e:?}");
        assert_eq!(e.details["reason"], reasons::REMOVED, "{e:?}");
        assert_eq!(e.details["againstTx"], "b1");
    }
}

/// The undo of a ripple move (its `item.remove` and ripple `item.restore`) follows the moved
/// left neighbour too.
#[test]
fn undo_of_ripple_move_follows_its_moved_left_neighbour() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10]));
    // alice moves c2 to the end: c1 c3 c4 c2.
    log.commit(
        "main",
        "e1",
        "alice",
        vec![move_to("c2", "t1", secs(40), InsertMode::Ripple)],
    );
    // bob moves c1 between c3 and c4: c3 c1 c4 c2.
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c1", "t1", secs(20), InsertMode::Ripple)],
    );
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(order(&log.state("main")), ["c3", "c1", "c2", "c4"]);
    let p = one(&log.placements);
    assert_eq!((p.item.as_str(), p.outcome), ("c2", Outcome::Relocated));
    assert_eq!(p.causes[0].tx_id, "b1");
    assert_well_formed(&log.state("main"));
}

/// Decision 4: a relocated undo, its redo and the undo of that redo: each lands exactly where
/// the one before left the timeline, and the redo's own undo puts the item back where the
/// first undo did.
#[test]
fn redo_and_undo_again_land_where_the_undo_did() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10]));
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c1", "t1", secs(30), InsertMode::Ripple)],
    );
    let before = log.state("main");
    log.undo("main", "alice", "u1").unwrap();
    let undone = log.state("main");
    assert_eq!(order(&undone), ["c3", "c4", "c1", "c2"]);
    log.redo("main", "alice", "r1").unwrap();
    assert_eq!(log.state("main").timelines, before.timelines);
    // A redo of a delete puts nothing back.
    assert!(log.placements.is_empty(), "{:?}", log.placements);
    log.undo("main", "alice", "u2").unwrap();
    assert_eq!(log.state("main").timelines, undone.timelines);
    assert_eq!(one(&log.placements).outcome, Outcome::Clean);
}

/// Decision 1 for a copy: a cherry-picked ripple move goes right after the item it followed
/// on the source, wherever the target has moved that item.
#[test]
fn picked_move_goes_after_its_left_neighbour_on_the_target() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10]));
    log.fork("br", "main", 0);
    // br: alice moves c1 between c2 and c3: c2 c1 c3.
    let e = log.commit(
        "br",
        "e1",
        "alice",
        vec![move_to("c1", "t1", secs(20), InsertMode::Ripple)],
    );
    // main: bob moves c2 to the end: c1 c3 c2.
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c2", "t1", secs(30), InsertMode::Ripple)],
    );
    log.pick(&e.record.tx_id, "main", "alice", "p1")
        .unwrap()
        .unwrap();
    assert_eq!(order(&log.state("main")), ["c3", "c2", "c1"]);
    let p = one(&log.placements);
    assert_eq!((p.tx_id.as_str(), p.outcome), ("p1", Outcome::Relocated));
    assert_eq!(p.causes[0].neighbour, "c2");
    assert_eq!(p.causes[0].author.as_deref(), Some("bob"));
    assert_well_formed(&log.state("main"));
}

/// A merge reports a placement per copied move, with the copy's own txId.
#[test]
fn merge_reports_placements_per_copy() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10]));
    log.fork("br", "main", 0);
    log.commit(
        "br",
        "e1",
        "alice",
        vec![move_to("c1", "t1", secs(20), InsertMode::Ripple)],
    );
    let plan = log.merge("br", "main", "alice", "m1", false).unwrap();
    assert_eq!(order(&log.state("main")), ["c2", "c1", "c3"]);
    let p = one(&plan.placements);
    assert_eq!((p.tx_id.as_str(), p.outcome), ("m1-1", Outcome::Clean));
}

/// A stale commit keeps the start-first rule (D2): an insert at a cut goes before the item
/// that follows, wherever it is now, and reports no placements.
#[test]
fn copied_insert_keeps_the_start_rule() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10]));
    log.fork("br", "main", 0);
    log.commit(
        "br",
        "e1",
        "alice",
        vec![insert(
            "$x",
            "t1",
            secs(10),
            "a1",
            Rational::zero(),
            secs(1),
        )],
    );
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c2", "t1", secs(30), InsertMode::Ripple)],
    );
    let plan = log.merge("br", "main", "alice", "m1", false).unwrap();
    let head = log.state("main");
    let x = &plan.planned[0].applied.bindings["$x"];
    assert_eq!(order(&head), ["c1", "c3", x.as_str(), "c2"]);
    assert!(plan.placements.is_empty(), "{:?}", plan.placements);
}

/// Core review, finding 1: a copy of a record that inserts `$x` and then ripple-moves it names
/// the item by its real id, and a relocation says where it landed.
#[test]
fn copied_move_of_its_own_insert_names_the_real_item() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10]));
    log.fork("br", "main", 0);
    let e = log.commit(
        "br",
        "e1",
        "alice",
        vec![
            insert("$x", "t1", secs(40), "a1", Rational::zero(), secs(1)),
            move_to("$x", "t1", secs(10), InsertMode::Ripple),
        ],
    );
    let x = e.record.bindings["$x"].clone();
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c1", "t1", secs(40), InsertMode::Ripple)],
    );
    let plan = log.merge("br", "main", "alice", "m1", false).unwrap();
    let p = one(&plan.placements);
    assert_eq!(p.item, x);
    assert_eq!(p.outcome, Outcome::Relocated);
    let landed = p.landed.as_ref().expect("landed");
    assert_eq!(landed.after.as_deref(), Some("c1"));
    assert_eq!(
        order(&log.state("main")),
        ["c2", "c3", "c4", "c1", x.as_str()]
    );
}

/// Core review, findings 2 and 3: an agent's `undo --to` refusal names one op index in both
/// places, and its causes carry their authors.
#[test]
fn undo_to_refusal_names_its_op_and_authors() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10, 10, 10]));
    log.when_lost = WhenLost::Refuse;
    let cp = log.checkpoint("main", "k1", "alice", "start");
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    log.commit("main", "e2", "alice", vec![delete("c5")]);
    log.commit("main", "b1", "bob", vec![delete("c1"), delete("c3")]);
    let e = log.undo_to("main", "alice", cp, "u1").unwrap_err();
    let c = &e.details["conflicts"][0];
    assert_eq!(c["reason"], reasons::NEIGHBOURS_GONE, "{e:?}");
    assert_eq!(c["opIndex"], c["details"]["placement"]["opIndex"], "{c}");
    assert_eq!(
        c["details"]["placement"]["causes"][0]["author"], "bob",
        "{c}"
    );
}

/// Core review, finding 5: a left neighbour moved to another track is `moved`, not `removed`.
#[test]
fn neighbour_moved_to_another_track_is_moved() {
    let mut log = MemLog::new(common::moves::with_v2(with_clips(&[10, 10, 10])));
    log.commit("main", "e1", "alice", vec![delete("c2")]);
    log.commit(
        "main",
        "b1",
        "bob",
        vec![move_to("c1", "t3", secs(0), InsertMode::Ripple)],
    );
    log.undo("main", "alice", "u1").unwrap();
    let p = one(&log.placements);
    assert_eq!(p.outcome, Outcome::Relocated);
    let causes: Vec<_> = p
        .causes
        .iter()
        .map(|c| (c.neighbour.as_str(), c.change))
        .collect();
    assert_eq!(causes, [("c1", Change::Moved)]);
}
