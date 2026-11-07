//! The rebase engine (spec §4.3, Decisions 16–18): positions mapped op by op through
//! `[inverse(our earlier ops), intervening steps, our rebased earlier ops]`, entity conflicts
//! judged per entity, and each op re-run at the head with full policy.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;
use fin_core::rebase::{Rebase, Rebased, Step, reasons, rebase_apply, steps_from};
use fin_core::{
    ApplyMode, BoundIds, CommittedTx, CoreError, DeleteMode, Edge, Item, ItemRemove, ItemRestore,
    Link, MemIds, Op, Project, Rational, Tx, apply_committed, apply_internal, apply_tx, rebase_tx,
};
use proptest::prelude::*;
use serde_json::json;

/// Applies `ops` to `p` and returns the result with its committed record at `rev`.
fn commit(p: &Project, ops: Vec<Op>, rev: u64, tx_id: &str) -> (Project, CommittedTx) {
    commit_with(p, ops, rev, tx_id, &mut MemIds::after(p))
}

fn commit_with(
    p: &Project,
    ops: Vec<Op>,
    rev: u64,
    tx_id: &str,
    ids: &mut MemIds,
) -> (Project, CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = tx_id.into();
    t.base_rev = rev - 1;
    let a = if t.ops.iter().any(Op::is_internal) {
        apply_internal(p, &t, ids)
    } else {
        apply_tx(p, &t, ids)
    }
    .unwrap_or_else(|e| panic!("{tx_id} does not apply: {e}"));
    (
        a.project,
        CommittedTx {
            rev,
            tx: t,
            diff: a.diff,
            bindings: a.bindings,
            link: None,
        },
    )
}

fn stale(ops: Vec<Op>) -> Tx {
    let mut t = tx(ops);
    t.tx_id = "tx-b".into();
    t
}

/// `t` (written at `base`) rebased onto `head` over `since`, ids allocated past `head`.
fn rebase(
    base: &Project,
    head: &Project,
    t: &Tx,
    since: &[CommittedTx],
) -> Result<Rebased, CoreError> {
    rebase_tx(base, head, t, since, &mut MemIds::after(head))
}

fn at_of(op: &Op) -> Rational {
    match op {
        Op::ItemInsert(i) => i.at,
        other => panic!("not an insert: {other:?}"),
    }
}

/// The `CONFLICT` reason, op index and blamed tx of `e`, checking it lists one conflict.
fn conflict(e: &CoreError) -> (String, u64, Option<String>) {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    let all = e.details["conflicts"]
        .as_array()
        .expect("details.conflicts");
    assert_eq!(all.len(), 1, "{e:?}");
    assert_eq!(all[0]["reason"], e.details["reason"]);
    (
        e.details["reason"].as_str().unwrap().to_string(),
        e.details["opIndex"].as_u64().unwrap(),
        e.details["againstTx"].as_str().map(str::to_string),
    )
}

#[test]
fn rebase_insert_over_insert_same_track() {
    let p = with_clips(&[5]);
    let (pa, a) = commit(
        &p,
        vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(3))],
        1,
        "tx-a",
    );
    let b = stale(vec![
        insert("$y", "t1", secs(5), "a1", secs(0), secs(2)),
        insert("$z", "t1", secs(0), "a1", secs(0), secs(1)),
    ]);
    let rb = rebase(&p, &pa, &b, std::slice::from_ref(&a)).unwrap();
    assert_eq!(rb.tx.base_rev, 1);
    assert_eq!(
        at_of(&rb.tx.ops[0]),
        secs(8),
        "at >= earlier at shifts by the earlier duration"
    );
    assert_eq!(
        at_of(&rb.tx.ops[1]),
        secs(0),
        "at before the earlier insert stays"
    );
    let done = rb.applied.project;
    assert_well_formed(&done);
    assert_eq!(done.timelines[0].tracks[0].items.len(), 4);
    assert_eq!(try_apply(&pa, rb.tx.ops).unwrap().project, done);
}

#[test]
fn insert_on_another_track_is_unchanged() {
    let p = with_clips(&[5]);
    let (pa, a) = commit(
        &p,
        vec![insert("$x", "t1", secs(0), "a1", secs(0), secs(3))],
        1,
        "tx-a",
    );
    let b = stale(vec![insert("$y", "t2", secs(0), "a1", secs(0), secs(2))]);
    assert_eq!(rebase(&p, &pa, &b, &[a]).unwrap().tx.ops, b.ops);
}

#[test]
fn insert_after_a_deleted_clip_moves_left() {
    let p = with_clips(&[5, 3]);
    let (pa, a) = commit(&p, vec![delete("c1")], 1, "tx-a");
    let b = stale(vec![insert("$y", "t1", secs(8), "a1", secs(0), secs(2))]);
    let rb = rebase(&p, &pa, &b, &[a]).unwrap();
    assert_eq!(at_of(&rb.tx.ops[0]), secs(3));
    assert_well_formed(&rb.applied.project);
}

#[test]
fn edit_of_deleted_clip_conflicts() {
    let p = with_clips(&[5, 3]);
    let (pa, a) = commit(&p, vec![delete("c2")], 1, "tx-a");
    let b = stale(vec![
        insert("$y", "t1", secs(0), "a1", secs(0), secs(1)),
        trim("c2", Edge::End, secs(1)),
    ]);
    let e = rebase(&p, &pa, &b, std::slice::from_ref(&a)).unwrap_err();
    assert_eq!(
        conflict(&e),
        (reasons::REMOVED.to_string(), 1, Some("tx-a".to_string()))
    );
    assert_eq!(e.details["txId"], "tx-b");
    assert_eq!(e.details["conflicts"][0]["txId"], "tx-b");
    let del = stale(vec![delete("c2")]);
    let e = rebase(&p, &pa, &del, &[a]).unwrap_err();
    assert_eq!(conflict(&e).0, reasons::REMOVED);
}

#[test]
fn double_trim_on_the_same_edge_conflicts() {
    let p = with_clips(&[5, 3]);
    let (pa, a) = commit(&p, vec![trim("c1", Edge::End, secs(1))], 1, "tx-a");
    let same = stale(vec![trim("c1", Edge::End, secs(-1))]);
    let e = rebase(&p, &pa, &same, std::slice::from_ref(&a)).unwrap_err();
    assert_eq!(
        conflict(&e),
        (reasons::SAME_EDGE.to_string(), 0, Some("tx-a".to_string()))
    );
    let other = stale(vec![trim("c1", Edge::Start, secs(1))]);
    let done = rebase(&p, &pa, &other, &[a]).unwrap().applied.project;
    assert_eq!(
        (clip(&done, "c1").duration, clip(&done, "c2").start),
        (secs(5), secs(5))
    );
}

#[test]
fn later_ops_see_concurrent_shifts_through_earlier_inserts() {
    // B inserts 10s at 0 (c1 moves to 10) and then inserts at c1's new start (10).
    let p = with_clips(&[5]);
    let (pa, a) = commit(
        &p,
        vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(3))],
        1,
        "tx-a",
    );
    let b = stale(vec![
        insert("$y", "t1", secs(0), "a1", secs(0), secs(10)),
        insert("$z", "t1", secs(10), "a1", secs(0), secs(1)),
    ]);
    let rb = rebase(&p, &pa, &b, &[a]).unwrap();
    assert_eq!(
        (at_of(&rb.tx.ops[0]), at_of(&rb.tx.ops[1])),
        (secs(0), secs(10))
    );
    let done = rb.applied;
    assert_well_formed(&done.project);
    let z = &done.bindings["$z"];
    assert_eq!(clip(&done.project, z).start, secs(10));
    assert_eq!(
        clip(&done.project, "c1").start,
        secs(11),
        "z lands before c1, as B intended"
    );
}

/// 4A refused this ("position follows a trim or delete on a track that moved"); the exact maps
/// carry the position through our own delete.
#[test]
fn positions_after_our_own_trim_or_delete_follow_the_track() {
    let p = with_clips(&[5, 3]);
    let (pa, a) = commit(
        &p,
        vec![insert("$x", "t1", secs(0), "a1", secs(0), secs(3))],
        1,
        "tx-a",
    );
    // At the base, after deleting c1, c2 is [0, 3): insert at its end.
    let b = stale(vec![
        delete("c1"),
        insert("$y", "t1", secs(3), "a1", secs(0), secs(1)),
    ]);
    let rb = rebase(&p, &pa, &b, std::slice::from_ref(&a)).unwrap();
    // At the head: x [0, 3), c1 [3, 8), c2 [8, 11); after the delete c2 is [3, 6).
    assert_eq!(at_of(&rb.tx.ops[1]), secs(6));
    let y = &rb.applied.bindings["$y"];
    assert_eq!(
        starts(&rb.applied.project, 0),
        vec![
            (a.bindings["$x"].clone(), secs(0)),
            ("c2".to_string(), secs(3)),
            (y.clone(), secs(6)),
        ]
    );
    // Our own trim, then a position past it: the trim's delta is carried.
    let b = stale(vec![
        trim("c1", Edge::End, secs(-2)),
        insert("$y", "t1", secs(6), "a1", secs(0), secs(1)),
    ]);
    let rb = rebase(&p, &pa, &b, &[a]).unwrap();
    // Head: x [0, 3), c1 [3, 6) after the trim, c2 [6, 9): end of c2 is 9.
    assert_eq!(at_of(&rb.tx.ops[1]), secs(9));
}

#[test]
fn insert_before_own_earlier_insert() {
    // c1 [0, 5), c2 [5, 10): A at the track end (10), then B at 10, before A.
    let p = with_clips(&[5, 5]);
    let (pa, a) = commit(
        &p,
        vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(2))],
        1,
        "tx-a",
    );
    let b = stale(vec![
        insert("$a", "t1", secs(10), "a1", secs(0), secs(3)),
        insert("$b", "t1", secs(10), "a1", secs(0), secs(4)),
    ]);
    let rb = rebase(&p, &pa, &b, &[a]).unwrap();
    let ids = &rb.applied.bindings;
    let done = &rb.applied.project;
    assert_eq!(clip(done, &ids["$b"]).start, secs(12));
    assert_eq!(clip(done, &ids["$a"]).start, secs(16), "12 + B's duration");
    assert_well_formed(done);
}

#[test]
fn only_newer_revisions_apply_in_rev_order() {
    let p = with_clips(&[5]);
    let mut ids = MemIds::after(&p);
    let (p1, c1) = commit_with(
        &p,
        vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(3))],
        1,
        "tx-1",
        &mut ids,
    );
    let (p2, c2) = commit_with(
        &p1,
        vec![insert("$x", "t1", secs(0), "a1", secs(0), secs(2))],
        2,
        "tx-2",
        &mut ids,
    );
    let mut b = stale(vec![insert("$y", "t1", secs(5), "a1", secs(0), secs(1))]);
    let all = rebase(&p, &p2, &b, &[c2.clone(), c1.clone()]).unwrap();
    assert_eq!((all.tx.base_rev, at_of(&all.tx.ops[0])), (2, secs(10)));
    b.base_rev = 1;
    b.ops = vec![insert("$y", "t1", secs(8), "a1", secs(0), secs(1))];
    let newer = rebase(&p1, &p2, &b, &[c1, c2]).unwrap();
    assert_eq!((newer.tx.base_rev, at_of(&newer.tx.ops[0])), (2, secs(10)));
    let none = rebase(&p1, &p1, &b, &[]).unwrap();
    assert_eq!(none.tx, b);
    assert_eq!(none.applied, try_apply(&p1, b.ops.clone()).unwrap());
}

/// The TASKS follow-up: a stale insert of an asset removed since is `CONFLICT` `REMOVED`, not
/// the head's `NOT_FOUND`.
#[test]
fn stale_insert_of_removed_asset_conflicts_removed() {
    let p = apply(&with_clips(&[5]), vec![av_asset("$b", secs(10))]).project;
    let (pa, a) = commit(&p, vec![Op::AssetRemove { id: "a2".into() }], 1, "tx-a");
    let b = stale(vec![insert("$y", "t1", secs(5), "a2", secs(0), secs(1))]);
    let e = rebase(&p, &pa, &b, &[a]).unwrap_err();
    assert_eq!(
        conflict(&e),
        (reasons::REMOVED.to_string(), 0, Some("tx-a".to_string()))
    );
}

/// A tx that does not apply at its own base still meets the conflict rules first, as 4A did
/// (`fin --idem-key` sends a locally invalid step so the server judges it): its clip deleted
/// since is `REMOVED`; without a conflict the base's own error stands.
#[test]
fn invalid_at_base_but_removed_since_is_conflict() {
    let p = with_clips(&[2, 3]);
    let (pa, a) = commit(&p, vec![delete("c1")], 1, "tx-a");
    let too_far = trim("c1", Edge::End, secs(-10));
    let b = stale(vec![
        insert("$y", "t1", secs(5), "a1", secs(0), secs(1)),
        too_far.clone(),
    ]);
    let e = rebase(&p, &pa, &b, std::slice::from_ref(&a)).unwrap_err();
    assert_eq!(
        conflict(&e),
        (reasons::REMOVED.to_string(), 1, Some("tx-a".to_string()))
    );
    let b = stale(vec![trim("c2", Edge::End, secs(-10))]);
    let e = rebase(&p, &pa, &b, std::slice::from_ref(&a)).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["opIndex"].as_u64()),
        ("INVALID_RANGE", Some(0))
    );
    assert_eq!(e.details["baseRev"], 0);
    assert!(e.fix.as_deref().unwrap().contains("current head"), "{e:?}");
    // An earlier op of ours that conflicts is named first.
    let b = stale(vec![delete("c1"), too_far]);
    let e = rebase(&p, &pa, &b, &[a]).unwrap_err();
    assert_eq!(conflict(&e).1, 0);
}

/// A committed restore without ripple fills a gap around our insert position: the re-run at
/// the head fails `INSERT_INSIDE_CLIP`, reported as the conflict's reason.
#[test]
fn insert_inside_committed_clip_is_policy_conflict() {
    let p = with_clips(&[5]);
    let placed = Item::clip("c9", "a1", secs(6), secs(0), secs(3));
    let (pa, a) = commit(
        &p,
        vec![
            ItemRestore {
                item: placed,
                track: "t1".into(),
                mode: DeleteMode::Lift,
            }
            .into(),
        ],
        1,
        "tx-a",
    );
    let b = stale(vec![insert("$y", "t1", secs(7), "a1", secs(0), secs(1))]);
    let e = rebase(&p, &pa, &b, &[a]).unwrap_err();
    // The step that placed c9 is blamed, and the policy error's details are kept.
    assert_eq!(
        conflict(&e),
        (
            "INSERT_INSIDE_CLIP".to_string(),
            0,
            Some("tx-a".to_string())
        )
    );
    assert_eq!(e.details["conflicts"][0]["details"]["clip"], "c9");
    assert!(
        e.details["conflicts"][0]["details"]
            .get("opIndex")
            .is_none()
    );
}

/// A re-run that fails policy keeps the error's details; the one step that changed the clip
/// is blamed.
#[test]
fn rerun_policy_conflict_keeps_details_and_blames_the_step() {
    let p = with_clips(&[5, 3]);
    let mut ids = MemIds::after(&p);
    let (p1, a) = commit_with(
        &p,
        vec![trim("c1", Edge::Start, secs(2))],
        1,
        "tx-a",
        &mut ids,
    );
    let (p2, other) = commit_with(
        &p1,
        vec![insert("$x", "t2", secs(0), "a1", secs(0), secs(1))],
        2,
        "tx-other",
        &mut ids,
    );
    // Valid at the base (c1 is 5 s), too far at the head (c1 is 3 s).
    let b = stale(vec![trim("c1", Edge::End, secs(-3))]);
    let e = rebase_tx(&p, &p2, &b, &[a, other], &mut ids).unwrap_err();
    assert_eq!(
        conflict(&e),
        ("INVALID_RANGE".to_string(), 0, Some("tx-a".to_string()))
    );
    let details = &e.details["conflicts"][0]["details"];
    assert_eq!(details["clip"], "c1");
    assert!(details.get("min").is_some() && details.get("max").is_some());
}

#[test]
fn other_track_edits_never_conflict() {
    let p = apply(
        &with_clips(&[5, 3]),
        vec![insert("$u", "t2", secs(0), "a1", secs(0), secs(4))],
    )
    .project;
    let mut ids = MemIds::after(&p);
    let (p1, c1) = commit_with(&p, vec![delete("c1")], 1, "tx-1", &mut ids);
    let (p2, c2) = commit_with(
        &p1,
        vec![
            trim("c2", Edge::End, secs(2)),
            insert("$x", "t1", secs(0), "a1", secs(0), secs(7)),
        ],
        2,
        "tx-2",
        &mut ids,
    );
    let b = stale(vec![
        trim("c3", Edge::End, secs(1)),
        insert("$y", "t2", secs(5), "a1", secs(0), secs(2)),
        trim("c3", Edge::Start, secs(1)),
    ]);
    let rb = rebase_tx(&p, &p2, &b, &[c1, c2], &mut ids).unwrap();
    assert_eq!(rb.tx.ops, b.ops);
    assert_eq!(starts(&rb.applied.project, 0), starts(&p2, 0));
}

#[test]
fn restore_with_placeholder_id_binds() {
    let p = with_clips(&[5]);
    let restore: Op = ItemRestore {
        item: Item::clip("$r", "a1", secs(5), secs(0), secs(2)),
        track: "t1".into(),
        mode: DeleteMode::Ripple,
    }
    .into();
    let t = tx(vec![restore.clone(), trim("$r", Edge::End, secs(1))]);
    assert_eq!(
        apply_tx(&p, &t, &mut MemIds::after(&p)).unwrap_err().code,
        "INVALID_OP"
    );
    let live = apply_internal(&p, &t, &mut MemIds::after(&p)).unwrap();
    assert_eq!(live.bindings["$r"], "c2");
    assert_eq!(clip(&live.project, "c2").duration, secs(3));
    assert_eq!(
        live.inverse,
        vec![
            trim_back("c2", Edge::End, secs(-1)),
            ItemRemove {
                id: "c2".into(),
                mode: DeleteMode::Ripple
            }
            .into(),
        ]
    );
    let replayed = apply_committed(&p, &t, &live.bindings).unwrap();
    assert_eq!(replayed, live);
    // Replay hands the bound id out again, whatever the counters would say.
    let other: BTreeMap<String, String> = [("$r".to_string(), "c7".to_string())].into();
    let replayed = apply_committed(&p, &t, &other).unwrap();
    assert_eq!(clip(&replayed.project, "c7").start, secs(5));
}

/// Decision 18: a copy re-run with the source's bindings meets an entity of the same id. Bound
/// by a record of the same origin chain it is `ALREADY_PRESENT`; otherwise `ID_COLLISION`.
#[test]
fn copy_with_bound_ids_duplicate_is_already_present() {
    let p = with_clips(&[5]);
    let (_, source) = commit(
        &p,
        vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(2))],
        1,
        "src-1",
    );
    // The target already holds the copy (c2) from an earlier merge of `src-1`.
    let target = apply_committed(&p, &source.tx, &source.bindings)
        .unwrap()
        .project;
    let step_free: Vec<Step> = Vec::new();
    let run = |same_origin: &BTreeSet<String>| {
        let r = Rebase {
            tx: &source.tx,
            own: &source.diff,
            steps: &step_free,
            same_origin,
            env: Default::default(),
            absorbs: &std::collections::BTreeSet::new(),
        };
        let mut ids = BoundIds::new(&source.tx.ops, &source.bindings).unwrap();
        rebase_apply(&target, &r, &mut ids, ApplyMode::Internal).unwrap_err()
    };
    let present = run(&["c2".to_string()].into());
    assert_eq!(
        (
            present.reason.as_str(),
            present.op_index,
            present.tx_id.as_str()
        ),
        (reasons::ALREADY_PRESENT, 0, "src-1")
    );
    assert_eq!(run(&BTreeSet::new()).reason, reasons::ID_COLLISION);
    // Without the copy, the re-run binds the source's id.
    let r = Rebase {
        tx: &source.tx,
        own: &source.diff,
        steps: &step_free,
        same_origin: &BTreeSet::new(),
        env: Default::default(),
        absorbs: &std::collections::BTreeSet::new(),
    };
    let mut ids = BoundIds::new(&source.tx.ops, &source.bindings).unwrap();
    let copied = rebase_apply(&p, &r, &mut ids, ApplyMode::Internal).unwrap();
    assert_eq!(copied.applied.bindings, source.bindings);
}

/// A step and its mirror (an undo of it, here) cancel: neither is judged for conflicts.
#[test]
fn mirrored_steps_are_not_judged() {
    let p = with_clips(&[5, 3]);
    let (p1, t) = commit(&p, vec![trim("c1", Edge::End, secs(1))], 1, "tx-trim");
    let (p2, undo) = commit(&p1, vec![trim("c1", Edge::End, secs(-1))], 2, "tx-undo");
    assert_eq!(p2, p);
    let mut steps = vec![
        Step::from_committed(&t).unwrap(),
        Step::from_committed(&undo).unwrap(),
    ];
    let b = stale(vec![
        trim("c1", Edge::End, secs(-1)),
        insert("$y", "t1", secs(4), "a1", secs(0), secs(1)),
    ]);
    let own = apply_tx(&p, &b, &mut MemIds::after(&p)).unwrap().diff;
    let empty = BTreeSet::new();
    let unmirrored = Rebase {
        tx: &b,
        own: &own,
        steps: &steps,
        same_origin: &empty,
        env: Default::default(),
        absorbs: &std::collections::BTreeSet::new(),
    };
    let e = rebase_apply(&p2, &unmirrored, &mut MemIds::after(&p2), ApplyMode::Client).unwrap_err();
    assert_eq!(
        (e.reason.as_str(), e.against_tx.as_deref()),
        (reasons::SAME_EDGE, Some("tx-trim"))
    );
    steps[1].mirror_of = Some(0);
    let mirrored = Rebase {
        tx: &b,
        own: &own,
        steps: &steps,
        same_origin: &empty,
        env: Default::default(),
        absorbs: &std::collections::BTreeSet::new(),
    };
    let done = rebase_apply(&p2, &mirrored, &mut MemIds::after(&p2), ApplyMode::Client).unwrap();
    assert_eq!(done.tx.ops, b.ops);
    assert_eq!(clip(&done.applied.project, "c1").duration, secs(4));
}

/// A stale tx that names an entity added since fails at its base: the error says which base
/// rev it was judged at and to send the edit again against the head.
#[test]
fn stale_insert_of_asset_added_since_names_the_base() {
    let p = with_clips(&[5]);
    let (p1, add) = commit(&p, vec![av_asset("$b", secs(50))], 1, "tx-add");
    let b = stale(vec![insert("$y", "t1", secs(5), "a2", secs(0), secs(1))]);
    let e = rebase(&p, &p1, &b, &[add]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["baseRev"]),
        ("NOT_FOUND", &json!(0))
    );
    assert!(e.fix.as_deref().unwrap().contains("current head"), "{e:?}");
}

/// `committed` with a link, as an undo or redo record carries it.
fn linked(mut c: CommittedTx, link: Link) -> CommittedTx {
    c.link = Some(Box::new(link));
    c
}

fn undo_of(units: &[&str]) -> Link {
    Link::Undo {
        undoes: units.iter().map(|u| u.to_string()).collect(),
    }
}

/// The inverse ops of `ops` applied at `p` (the record an undo of them writes).
fn inverse(p: &Project, ops: &[Op]) -> Vec<Op> {
    apply_internal(p, &tx(ops.to_vec()), &mut MemIds::after(p))
        .unwrap()
        .inverse
}

/// The reviewer's counter-example: a delete and its undo between the base and the head. The
/// undo mirrors the delete, so "before c2" stays before c2, as at the head.
#[test]
fn undo_in_since_mirrors_its_unit() {
    let p = with_clips(&[5, 3]);
    let del = vec![delete("c2")];
    let (p1, u) = commit(&p, del.clone(), 1, "tx-del");
    let (p2, undo) = commit(&p1, inverse(&p, &del), 2, "tx-undo");
    assert_eq!(p2, p);
    let undo = linked(undo, undo_of(&["tx-del"]));
    let b = stale(vec![insert("$y", "t1", secs(5), "a1", secs(0), secs(1))]);
    let out = rebase(&p, &p2, &b, &[u, undo]).unwrap();
    let fresh = apply_tx(&p2, &b, &mut MemIds::after(&p2)).unwrap();
    assert_eq!(at_of(&out.tx.ops[0]), secs(5));
    assert_eq!(out.applied.project, fresh.project);
}

/// An `undo --to` of two units is cut into one part per unit, each mirroring its unit.
#[test]
fn undo_of_several_units_mirrors_each() {
    let p = with_clips(&[5, 3]);
    let del = vec![delete("c2")];
    let (p1, u1) = commit(&p, del.clone(), 1, "tx-1");
    let ins = vec![insert("$z", "t1", secs(0), "a1", secs(0), secs(1))];
    let (p2, u2) = commit(&p1, ins, 2, "tx-2");
    // Newest unit first: remove z, then restore c2.
    let resolved = fin_core::resolve_ops(&u2.tx.ops, &u2.bindings).unwrap();
    let mut ops = inverse(&p1, &resolved);
    ops.extend(inverse(&p, &del));
    let (p3, undo) = commit(&p2, ops, 3, "tx-undo");
    assert_eq!(p3, p);
    let undo = linked(undo, undo_of(&["tx-2", "tx-1"]));
    let since = [u1, u2, undo];
    let refs: Vec<&CommittedTx> = since.iter().collect();
    let steps = steps_from(&refs).unwrap();
    let mirrors: Vec<Option<usize>> = steps.iter().map(|s| s.mirror_of).collect();
    assert_eq!(mirrors, vec![None, None, Some(1), Some(0)]);
    let b = stale(vec![insert("$y", "t1", secs(5), "a1", secs(0), secs(1))]);
    let out = rebase(&p, &p3, &b, &since).unwrap();
    assert_eq!(at_of(&out.tx.ops[0]), secs(5));
}

/// The undo's unit is before the base; the redo mirrors the undo. Without the mirror, "before
/// x" (the start edge of x, removed then restored) would land after x.
#[test]
fn redo_mirrors_its_undo() {
    let p = with_clips(&[5, 3]);
    let ins = vec![insert("$x", "t1", secs(5), "a1", secs(0), secs(2))];
    let (base, u) = commit(&p, ins, 1, "tx-ins");
    let resolved = fin_core::resolve_ops(&u.tx.ops, &u.bindings).unwrap();
    let removal = inverse(&p, &resolved);
    let (p2, undo) = commit(&base, removal.clone(), 2, "tx-undo");
    let (head, redo) = commit(&p2, inverse(&base, &removal), 3, "tx-redo");
    assert_eq!(head, base);
    let since = [
        linked(undo, undo_of(&["tx-ins"])),
        linked(
            redo,
            Link::Redo {
                redoes: "tx-undo".into(),
            },
        ),
    ];
    let refs: Vec<&CommittedTx> = since.iter().collect();
    let mirrors: Vec<Option<usize>> = steps_from(&refs)
        .unwrap()
        .iter()
        .map(|s| s.mirror_of)
        .collect();
    assert_eq!(mirrors, vec![None, Some(0)]);
    let mut b = stale(vec![insert("$y", "t1", secs(5), "a1", secs(0), secs(1))]);
    b.base_rev = 1;
    let out = rebase(&base, &head, &b, &since).unwrap();
    assert_eq!(at_of(&out.tx.ops[0]), secs(5), "before x");
    // An undo whose unit is not here, or a link of another kind, stands alone.
    let lone = [linked(
        since[1].clone(),
        Link::Redo {
            redoes: "tx-elsewhere".into(),
        },
    )];
    let refs: Vec<&CommittedTx> = lone.iter().collect();
    assert_eq!(steps_from(&refs).unwrap()[0].mirror_of, None);
}

#[derive(Debug, Clone)]
enum Edit {
    Insert {
        boundary: usize,
        len: i64,
    },
    Trim {
        clip: usize,
        start: bool,
        frames: i64,
    },
    Delete {
        clip: usize,
    },
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0usize..12, 1i64..150).prop_map(|(boundary, len)| Edit::Insert { boundary, len }),
        (0usize..6, any::<bool>(), -60i64..60).prop_map(|(clip, start, frames)| Edit::Trim {
            clip,
            start,
            frames
        }),
        (0usize..6).prop_map(|clip| Edit::Delete { clip }),
    ]
}

fn to_op(p: &Project, e: &Edit, name: &str) -> Op {
    let items = &p.timelines[0].tracks[0].items;
    let f = |n: i64| Rational::new(n, 30).unwrap();
    match *e {
        Edit::Insert { boundary, len } => {
            let at = match items.get(boundary / 2) {
                Some(c) if boundary % 2 == 0 => c.start,
                Some(c) => (c.start + c.duration).unwrap(),
                None => items
                    .last()
                    .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap()),
            };
            insert(name, "t1", at, "a1", secs(10), f(len))
        }
        Edit::Trim {
            clip,
            start,
            frames,
        } => trim(
            &items[clip % items.len()].id,
            if start { Edge::Start } else { Edge::End },
            f(frames),
        ),
        Edit::Delete { clip } => delete(&items[clip % items.len()].id),
    }
}

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

proptest! {
    #![proptest_config(config(256))]
    #[test]
    fn rebased_pairs_never_overlap_and_keep_both_edits(
        lens in prop::collection::vec(1i64..6, 1..5),
        ea in edit(),
        eb in edit(),
    ) {
        let p = with_clips(&lens);
        let (oa, ob) = (to_op(&p, &ea, "$a"), to_op(&p, &eb, "$b"));
        // Skipped, not rejected: proptest aborts after too many global rejects at high counts.
        if try_apply(&p, vec![oa.clone()]).is_err() || try_apply(&p, vec![ob.clone()]).is_err() {
            return Ok(());
        }
        // One allocator for both, like the store's durable counter (ids are never reused).
        let mut ids = MemIds::after(&p);
        let (pa, a) = commit_with(&p, vec![oa.clone()], 1, "tx-a", &mut ids);
        match rebase_tx(&p, &pa, &stale(vec![ob.clone()]), std::slice::from_ref(&a), &mut ids) {
            Ok(rb) => {
                let done = rb.applied;
                assert_well_formed(&done.project);
                if let Op::ItemInsert(_) = oa {
                    clip(&done.project, &a.bindings["$a"]);
                }
                if let Op::ItemInsert(i) = &ob {
                    prop_assert_eq!(clip(&done.project, &done.bindings["$b"]).duration, i.duration);
                }
                if let Op::ItemDelete(fin_core::ItemDelete { item: id, .. }) = &oa {
                    prop_assert!(done.project.timelines[0].tracks[0].items.iter().all(|c| &c.id != id));
                }
            }
            // Entity conflicts, or two trims of one clip that jointly exceed its bounds (the
            // re-run says so).
            Err(e) => {
                prop_assert_eq!(e.code.as_str(), "CONFLICT");
                let reason = e.details["reason"].as_str().unwrap_or_default();
                prop_assert!(
                    [reasons::REMOVED, reasons::SAME_EDGE].contains(&reason)
                        || (reason == "INVALID_RANGE" && matches!(ob, Op::ItemTrim(_))),
                    "{:?} over {:?}: {:?}", ob, oa, e
                );
            }
        }
    }
}

/// Builds a stale multi-op tx from `edits`, each written against the project as changed by the
/// earlier ones (clips created earlier in the tx are referenced by their placeholders).
fn stale_sequence(p: &Project, edits: &[Edit]) -> Option<Vec<Op>> {
    let mut local = p.clone();
    let mut ids = MemIds::after(p);
    let mut names = BTreeMap::<String, String>::new();
    let mut ops = Vec::new();
    for (i, e) in edits.iter().enumerate() {
        if local.timelines[0].tracks[0].items.is_empty() && !matches!(e, Edit::Insert { .. }) {
            continue;
        }
        let mut op = to_op(&local, e, &format!("$b{i}"));
        let a = apply_tx(&local, &tx(vec![op.clone()]), &mut ids).ok()?;
        if let Op::ItemTrim(fin_core::ItemTrim { item: clip, .. })
        | Op::ItemDelete(fin_core::ItemDelete { item: clip, .. }) = &mut op
            && let Some(name) = names.get(clip.as_str())
        {
            *clip = name.clone();
        }
        names.extend(a.bindings.into_iter().map(|(k, v)| (v, k)));
        local = a.project;
        ops.push(op);
    }
    Some(ops)
}

proptest! {
    #![proptest_config(config(256))]
    #[test]
    fn rebased_sequences_stay_coherent(
        lens in prop::collection::vec(1i64..6, 1..5),
        ea in prop::collection::vec(edit(), 1..3),
        eb in prop::collection::vec(edit(), 1..4),
    ) {
        let p = with_clips(&lens);
        let (Some(oa), Some(ob)) = (stale_sequence(&p, &ea), stale_sequence(&p, &eb)) else { return Ok(()) };
        if oa.is_empty() || try_apply(&p, ob.clone()).is_err() {
            return Ok(());
        }
        let mut ids = MemIds::after(&p);
        let (pa, a) = commit_with(&p, oa, 1, "tx-a", &mut ids);
        match rebase_tx(&p, &pa, &stale(ob.clone()), std::slice::from_ref(&a), &mut ids) {
            Ok(rb) => {
                assert_well_formed(&rb.applied.project);
                // The stored intent replays to the same state.
                let replayed = apply_committed(&pa, &rb.tx, &rb.applied.bindings).unwrap();
                prop_assert_eq!(replayed.project, rb.applied.project);
            }
            Err(e) => prop_assert_eq!(e.code.as_str(), "CONFLICT", "{:?} over {:?}: {:?}", ob, a.tx.ops, e),
        }
    }
}

/// Our stale tx inserts `$c` (bound to `c3` at the base) and trims it; meanwhile intervening
/// records allocated `c3` for another clip, trimmed its end and deleted it. Our footprints
/// name `$c`, never the base's `c3`, so the other clip is not ours and nothing conflicts:
/// ours lands under a fresh id, trimmed, and the other clip stays deleted.
#[test]
fn intervening_record_allocating_our_base_id_is_not_ours() {
    let p = with_clips(&[5, 5]);
    let mut ids = MemIds::after(&p);
    let (p1, a) = commit_with(
        &p,
        vec![insert("$x", "t2", secs(0), "a1", secs(0), secs(4))],
        1,
        "tx-a",
        &mut ids,
    );
    assert_eq!(a.bindings["$x"], "c3", "the id our base binds too");
    let (p2, b) = commit_with(
        &p1,
        vec![trim("c3", Edge::End, secs(-1))],
        2,
        "tx-trim",
        &mut ids,
    );
    let (p3, c) = commit_with(&p2, vec![delete("c3")], 3, "tx-del", &mut ids);
    let ours = stale(vec![
        insert("$c", "t1", secs(10), "a1", secs(0), secs(3)),
        trim("$c", Edge::End, secs(-1)),
    ]);
    let r = rebase_tx(&p, &p3, &ours, &[a, b, c], &mut ids).unwrap();
    let id = &r.applied.bindings["$c"];
    assert_ne!(id, "c3");
    let mine = clip(&r.applied.project, id);
    assert_eq!((mine.start, mine.duration), (secs(10), secs(2)));
    assert!(
        r.applied.project.timelines[0].tracks[1].items.is_empty(),
        "the other c3 stays deleted"
    );
}
