//! Moves, slides and link groups in the rebase engine (plan 4C Task 9): footprints from each
//! op's own diff never skip by track, and a move's `at` maps over its target track.

mod common;

use common::moves::*;
use common::*;
use fin_core::{
    CommittedTx, CoreError, Edge, InsertMode, MemIds, Op, Project, Tx, apply_tx, rebase_tx,
};

fn commit(p: &Project, ops: Vec<Op>) -> (Project, CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = "tx-a".into();
    let a = apply_tx(p, &t, &mut MemIds::after(p)).unwrap();
    let c = CommittedTx {
        rev: 1,
        tx: t,
        diff: a.diff,
        bindings: a.bindings,
        link: None,
    };
    (a.project, c)
}

/// Their `theirs` committed on `p`, then our `ours` written at `p` rebased over it.
fn rebase(
    p: &Project,
    theirs: Vec<Op>,
    ours: Vec<Op>,
) -> Result<fin_core::rebase::Rebased, CoreError> {
    let (head, c) = commit(p, theirs);
    let mut t: Tx = tx(ours);
    t.tx_id = "tx-b".into();
    rebase_tx(p, &head, &t, &[c], &mut MemIds::after(&head))
}

fn conflict(e: &CoreError) -> (String, Option<String>) {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    (
        e.details["reason"].as_str().unwrap().to_string(),
        e.details["againstTx"].as_str().map(str::to_string),
    )
}

#[test]
fn move_vs_concurrent_trim_composes() {
    let p = with_clips(&[4, 4, 4]);
    let r = rebase(
        &p,
        vec![trim("c1", Edge::End, secs(-1))],
        vec![move_to("c1", "t1", secs(12), InsertMode::Ripple)],
    )
    .unwrap();
    assert_eq!(
        r.tx.ops,
        [move_to("c1", "t1", secs(11), InsertMode::Ripple)]
    );
    assert_eq!(
        spans_on(&r.applied.project, 0),
        [sp("c2", 0, 4), sp("c3", 4, 4), sp("c1", 8, 3)]
    );
}

#[test]
fn move_vs_delete_removed() {
    let p = with_clips(&[4, 4]);
    let e = rebase(
        &p,
        vec![delete("c1")],
        vec![move_to("c1", "t1", secs(8), InsertMode::Ripple)],
    )
    .unwrap_err();
    assert_eq!(conflict(&e), ("REMOVED".into(), Some("tx-a".into())));
}

/// Entity rules never skip by track: an overwrite move onto another track meets the edits
/// made there to what it carves.
#[test]
fn move_to_other_track_meets_its_edits() {
    let p = apply(
        &with_v2(with_clips(&[4])),
        vec![insert("$z", "t3", secs(0), "a1", secs(0), secs(10))],
    )
    .project;
    let e = rebase(
        &p,
        vec![trim("c2", Edge::End, secs(-2))],
        vec![move_to("c1", "t3", secs(6), InsertMode::Overwrite)],
    )
    .unwrap_err();
    assert_eq!(conflict(&e), ("SAME_EDGE".into(), Some("tx-a".into())));
}

#[test]
fn stale_move_at_maps_over_target_track_insert() {
    let p = apply(
        &with_v2(with_clips(&[4])),
        vec![insert("$z", "t3", secs(0), "a1", secs(0), secs(10))],
    )
    .project;
    let r = rebase(
        &p,
        vec![insert("$y", "t3", secs(0), "a1", secs(0), secs(3))],
        vec![move_to("c1", "t3", secs(10), InsertMode::Ripple)],
    )
    .unwrap();
    assert_eq!(
        r.tx.ops,
        [move_to("c1", "t3", secs(13), InsertMode::Ripple)]
    );
    assert_eq!(
        spans_on(&r.applied.project, 2),
        [sp("c3", 0, 3), sp("c2", 3, 10), sp("c1", 13, 4)]
    );
}

#[test]
fn linked_delete_vs_member_trim_is_not_a_conflict() {
    let p = linked(10);
    let r = rebase(
        &p,
        vec![trim("c2", Edge::End, secs(-1))],
        vec![delete("c1")],
    )
    .unwrap();
    assert_eq!(spans_on(&r.applied.project, 0), [sp("c3", 0, 4)]);
    assert_eq!(spans_on(&r.applied.project, 1), [sp("c4", 0, 4)]);
    // The other way round, the member is gone.
    let e = rebase(
        &p,
        vec![delete("c1")],
        vec![trim("c2", Edge::End, secs(-1))],
    )
    .unwrap_err();
    assert_eq!(conflict(&e), ("REMOVED".into(), Some("tx-a".into())));
}

#[test]
fn slide_vs_slide_same_param() {
    let p = with_sourced(&[(10, 10), (10, 10), (10, 10)]);
    let e = rebase(&p, vec![slide("c2", secs(1))], vec![slide("c2", secs(-1))]).unwrap_err();
    assert_eq!(conflict(&e), ("SAME_PARAM".into(), Some("tx-a".into())));
}

#[test]
fn linked_move_undo_redo_round_trips() {
    let genesis = linked(10);
    let mut log = common::memlog::MemLog::new(genesis.clone());
    log.commit(
        "main",
        "e1",
        "alice",
        vec![move_to("c1", "t1", secs(8), InsertMode::Ripple)],
    );
    let moved = log.state("main");
    log.commit("main", "e2", "alice", vec![slide("c3", secs(1))]);
    log.undo("main", "alice", "u2").unwrap();
    log.undo("main", "alice", "u1").unwrap();
    assert_eq!(log.state("main"), genesis);
    log.redo("main", "alice", "r1").unwrap();
    assert_eq!(log.state("main"), moved);
}

/// 4C Tasks 9–10 review B1: a later op of ours at the start of an item our own tx moved stays
/// before it, whatever the head did to the moved item's length or its link group.
#[test]
fn own_move_then_insert_at_its_start_after_concurrent_trim() {
    let p = with_clips(&[4, 4]);
    let ours = vec![
        move_to("c1", "t1", secs(8), InsertMode::Ripple),
        insert("$n", "t1", secs(4), "a1", secs(0), secs(1)),
    ];
    for (theirs, len) in [(1, 5), (-1, 3)] {
        let r = rebase(&p, vec![trim("c1", Edge::End, secs(theirs))], ours.clone()).unwrap();
        let n = r.applied.bindings["$n"].clone();
        assert_eq!(
            spans_on(&r.applied.project, 0),
            [sp("c2", 0, 4), sp(&n, 4, 1), sp("c1", 5, len)],
            "their trim by {theirs}"
        );
    }
}

#[test]
fn own_move_to_another_track_then_insert_at_its_start() {
    let p = with_v2(with_clips(&[4, 4]));
    let ours = vec![
        move_to("c1", "t3", secs(0), InsertMode::Ripple),
        insert("$n", "t3", secs(0), "a1", secs(0), secs(1)),
    ];
    for (theirs, len) in [(1, 5), (-1, 3)] {
        let r = rebase(&p, vec![trim("c1", Edge::End, secs(theirs))], ours.clone()).unwrap();
        let n = r.applied.bindings["$n"].clone();
        assert_eq!(
            spans_on(&r.applied.project, 2),
            [sp(&n, 0, 1), sp("c1", 1, len)],
            "their trim by {theirs}"
        );
    }
}

#[test]
fn own_group_move_then_insert_after_concurrent_unlink() {
    // c1 [0,4) on t1 linked with c2 [0,4) on t2; c3 [4,8) on t1, c4 [4,8) on t2.
    let p = linked(10);
    for mode in [InsertMode::Ripple, InsertMode::Overwrite] {
        let ours = vec![
            move_to("c1", "t1", secs(8), mode),
            insert(
                "$n",
                "t1",
                secs(if mode == InsertMode::Ripple { 4 } else { 8 }),
                "a1",
                secs(0),
                secs(1),
            ),
        ];
        let r = rebase(&p, vec![unlink("lk1")], ours).unwrap();
        let n = r.applied.bindings["$n"].clone();
        let t1 = spans_on(&r.applied.project, 0);
        let at = |id: &str| t1.iter().position(|(i, ..)| i == id).unwrap();
        assert_eq!(at(&n) + 1, at("c1"), "{mode:?}: {t1:?}");
        // The unlinked partner stays where it was.
        assert_eq!(
            spans_on(&r.applied.project, 1)[0],
            sp("c2", 0, 4),
            "{mode:?}"
        );
    }
}
