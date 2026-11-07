//! `transition.set` / `transition.remove` and the post-op prune pass (plan 4C Task 10,
//! Decisions 22–25): transitions sit at cuts between touching items; any other op that leaves
//! a transition's items apart removes it and its undo restores it.

mod common;

use common::ids_json::ids_cover_every_id_in_json;
use common::memlog::MemLog;
use common::moves::*;
use common::*;
use fin_core::rebase::footprint;
use fin_core::rebase::{Touch, split_diff};
use fin_core::{
    ApplyMode, Change, ChangeKind, CommittedTx, CoreError, Curve, DeleteMode, Edge, EntityKind,
    InsertMode, ItemBody, ItemDelete, ItemInsert, ItemTrim, MemIds, Op, Project, Rational,
    Transition, TransitionKind, TransitionRemove, TransitionSet, TrimMode, Tx, apply_tx_mode,
    rebase_tx,
};

fn set(id: &str, a: &str, b: &str, duration: Rational) -> Op {
    TransitionSet {
        id: id.into(),
        track: "t1".into(),
        between: [a.into(), b.into()],
        kind: TransitionKind::Dissolve,
        duration,
        curve: Curve::Linear,
    }
    .into()
}

fn remove(id: &str) -> Op {
    TransitionRemove {
        transition: id.into(),
    }
    .into()
}

fn tr(id: &str, op: u32, change: ChangeKind) -> Change {
    Change {
        kind: EntityKind::Transition,
        id: id.into(),
        change,
        op,
        track: Some("t1".into()),
        edge: None,
        path: None,
        by: None,
    }
}

/// `(id, between)` of every transition on `t1`.
fn cuts(p: &Project) -> Vec<(String, [String; 2])> {
    p.timelines[0].tracks[0]
        .transitions
        .iter()
        .map(|t| (t.id.clone(), t.between.clone()))
        .collect()
}

fn cut(id: &str, a: &str, b: &str) -> (String, [String; 2]) {
    (id.into(), [a.into(), b.into()])
}

/// The transition changes of a diff.
fn tr_changes(d: &fin_core::Diff) -> Vec<(String, ChangeKind)> {
    d.changes
        .iter()
        .filter(|c| c.kind == EntityKind::Transition)
        .map(|c| (c.id.clone(), c.change))
        .collect()
}

fn replay(p: &Project, ops: Vec<Op>) -> Result<fin_core::Applied, CoreError> {
    apply_tx_mode(p, &tx(ops), &mut MemIds::after(p), ApplyMode::Replay)
}

fn code(r: Result<fin_core::Applied, CoreError>) -> String {
    r.unwrap_err().code
}

/// `t1`: `c1` [0,4) from 0, `c2` [4,8) from 10, `c3` [8,12) from 20 (asset `a1`, 100 s), so
/// every cut has handles.
fn clips() -> Project {
    with_sourced(&[(4, 0), (4, 10), (4, 20)])
}

/// [`clips`] with a 1 s dissolve `tr1` at the `c1`|`c2` cut.
fn dissolved() -> Project {
    let a = apply(&clips(), vec![set("$t", "c1", "c2", secs(1))]);
    assert_eq!(cuts(&a.project), [cut("tr1", "c1", "c2")]);
    a.project
}

fn the_transition(p: &Project) -> Transition {
    p.timelines[0].tracks[0].transitions[0].clone()
}

#[test]
fn set_creates_at_a_cut() {
    let p = clips();
    let a = golden_apply(&p, vec![set("$t", "c1", "c2", secs(1))]);
    assert_eq!(a.bindings["$t"], "tr1");
    assert_eq!(
        the_transition(&a.project),
        Transition {
            id: "tr1".into(),
            between: ["c1".into(), "c2".into()],
            kind: TransitionKind::Dissolve,
            duration: secs(1),
            curve: Curve::Linear,
        }
    );
    assert_eq!(a.diff.changes, [tr("tr1", 0, ChangeKind::Added)]);
    assert!(a.diff.shifts.is_empty());
    assert_eq!(a.inverse, [remove("tr1")]);
    // Footprint: transition.out on A, transition.in on B, creates it, uses the track.
    let resolved = set("tr1", "c1", "c2", secs(1));
    let f = footprint(
        &resolved,
        &split_diff(std::slice::from_ref(&resolved), &a.diff).unwrap()[0],
    );
    assert_eq!(
        f.touches,
        [
            ("c1".to_string(), Touch::Param("transition.out".into())),
            ("c2".to_string(), Touch::Param("transition.in".into())),
        ]
    );
    assert_eq!(f.creates, ["tr1"]);
    assert_eq!(f.uses, ["t1"]);
    // A second one at another cut is kept in id order.
    let b = golden_apply(&a.project, vec![set("$u", "c2", "c3", r("1/2"))]);
    assert_eq!(
        cuts(&b.project),
        [cut("tr1", "c1", "c2"), cut("tr2", "c2", "c3")]
    );
}

#[test]
fn set_with_a_literal_id_updates() {
    let p = dissolved();
    let mut op = set("tr1", "c1", "c2", secs(2));
    if let Op::TransitionSet(s) = &mut op {
        s.curve = Curve::EqualPower;
    }
    let a = golden_apply(&p, vec![op]);
    let t = the_transition(&a.project);
    assert_eq!((t.duration, t.curve), (secs(2), Curve::EqualPower));
    assert_eq!(a.diff.changes, [tr("tr1", 0, ChangeKind::Updated)]);
    // The inverse sets the old values back.
    assert_eq!(a.inverse, [set("tr1", "c1", "c2", secs(1))]);
    // Its cut and track stay.
    assert_eq!(
        code(try_apply(&p, vec![set("tr1", "c2", "c3", secs(1))])),
        "INVALID_OP"
    );
}

#[test]
fn remove_takes_it_off() {
    let p = dissolved();
    let a = golden_apply(&p, vec![remove("tr1")]);
    assert!(cuts(&a.project).is_empty());
    assert_eq!(a.diff.changes, [tr("tr1", 0, ChangeKind::Removed)]);
    assert_eq!(a.inverse, [set("tr1", "c1", "c2", secs(1))]);
    let f = footprint(&remove("tr1"), &a.diff);
    assert_eq!(f.removes, ["tr1"]);
    assert_eq!(f.tracks.iter().collect::<Vec<_>>(), ["t1"]);
    assert_eq!(code(try_apply(&p, vec![remove("tr9")])), "NOT_FOUND");
}

#[test]
fn ops_carry_their_ids() {
    for op in [
        set("$t", "c1", "c2", secs(1)),
        set("tr1", "$a", "$b", secs(1)),
        remove("tr1"),
    ] {
        ids_cover_every_id_in_json(&op);
    }
    assert_eq!(
        set("$t", "c1", "c2", secs(1)).ids(),
        ["$t", "t1", "c1", "c2"]
    );
    assert_eq!(set("$t", "c1", "c2", secs(1)).allocates(), Some("$t"));
    assert_eq!(set("tr1", "c1", "c2", secs(1)).allocates(), None);
}

#[test]
fn set_refusals() {
    let p = dissolved();
    // One per cut.
    let e = try_apply(&p, vec![set("$u", "c1", "c2", secs(1))]).unwrap_err();
    assert_eq!(e.code, "TRANSITION_EXISTS");
    assert_eq!(e.details["transition"], "tr1");
    // Items that do not touch, in either order.
    for (a, b) in [("c1", "c3"), ("c2", "c1")] {
        assert_eq!(
            code(try_apply(&p, vec![set("$u", a, b, secs(1))])),
            "NOT_ADJACENT"
        );
    }
    // At least one frame, at most the shorter item; positive is structural.
    for d in ["1/60", "5"] {
        let e = try_apply(&p, vec![set("$u", "c2", "c3", r(d))]).unwrap_err();
        assert_eq!(e.code, "INVALID_RANGE");
        assert_eq!(e.details["max"], "4");
    }
    assert!(replay(&p, vec![set("$u", "c2", "c3", r("1/60"))]).is_ok());
    for d in ["0", "-1"] {
        assert_eq!(
            code(replay(&p, vec![set("$u", "c2", "c3", r(d))])),
            "INVALID_RANGE"
        );
    }
    // Dissolve on video, crossfade on audio.
    let mut fade = set("$u", "c2", "c3", secs(1));
    if let Op::TransitionSet(s) = &mut fade {
        s.kind = TransitionKind::Crossfade;
    }
    assert_eq!(code(try_apply(&p, vec![fade])), "TRACK_KIND_MISMATCH");
    // A literal id naming none is a restore: internal only.
    assert_eq!(
        code(try_apply(&clips(), vec![set("tr7", "c1", "c2", secs(1))])),
        "INVALID_OP"
    );
    // A locked track.
    let mut locked = p.clone();
    locked.timelines[0].tracks[0].lock = true;
    assert_eq!(
        code(try_apply(&locked, vec![remove("tr1")])),
        "TRACK_LOCKED"
    );
    assert_eq!(
        code(try_apply(&locked, vec![set("$u", "c2", "c3", secs(1))])),
        "TRACK_LOCKED"
    );
}

#[test]
fn handles_are_policy() {
    // c2 starts at its source's start: nothing before the cut for it to read.
    let p = with_sourced(&[(4, 0), (4, 0)]);
    let e = try_apply(&p, vec![set("$t", "c1", "c2", secs(1))]).unwrap_err();
    assert_eq!(e.code, "TRANSITION_NO_HANDLES");
    assert_eq!(e.details["item"], "c2");
    assert_eq!(e.details["side"], "in");
    assert_eq!(e.details["need"], "1/2");
    // c1 uses its asset up to the end: nothing after the cut.
    let q = with_sourced(&[(4, 96), (4, 10)]);
    let e = try_apply(&q, vec![set("$t", "c1", "c2", secs(1))]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["side"]),
        ("TRANSITION_NO_HANDLES", &"out".into())
    );
    // Replay applies what was committed.
    let a = replay(&p, vec![set("$t", "c1", "c2", secs(1))]).unwrap();
    assert_eq!(cuts(&a.project), [cut("tr1", "c1", "c2")]);
    // Exactly half the duration of handle is enough.
    let s = with_sourced(&[(4, 0), (4, 0)]);
    let mut s = s;
    if let ItemBody::Clip { source_in, .. } = &mut s.timelines[0].tracks[0].items[1].body {
        *source_in = r("1/2");
    }
    assert!(try_apply(&s, vec![set("$t", "c1", "c2", secs(1))]).is_ok());
}

#[test]
fn replay_rejects_non_adjacent_transition() {
    let p = clips();
    let e = replay(&p, vec![set("$t", "c1", "c3", secs(1))]).unwrap_err();
    assert_eq!(e.code, "NOT_ADJACENT");
    // The structural check after each op of a replay: a transition left invalid on a track the
    // op touched (here, two at one cut, which only a damaged history could hold).
    let mut bad = dissolved();
    let mut twin = the_transition(&bad);
    twin.id = "tr5".into();
    bad.timelines[0].tracks[0].transitions.push(twin);
    let e = replay(&bad, vec![trim("c3", Edge::End, secs(-1))]).unwrap_err();
    assert_eq!(e.code, "TRANSITION_INVALID");
    assert_eq!(e.details["transition"], "tr5");
    // An edit elsewhere does not look at t1.
    assert!(
        replay(
            &bad,
            vec![insert("$n", "t2", secs(0), "a1", secs(0), secs(1))]
        )
        .is_ok()
    );
}

#[test]
fn ripple_delete_of_b_prunes_and_undo_restores() {
    let p = dissolved();
    let a = golden_apply(&p, vec![delete("c2")]);
    // c1 and c3 touch now, but the transition was between c1 and c2.
    assert!(cuts(&a.project).is_empty());
    assert_eq!(
        tr_changes(&a.diff),
        [("tr1".to_string(), ChangeKind::Removed)]
    );
    // The item's restore first, then the transition's.
    assert_eq!(a.inverse.len(), 2);
    assert_eq!(a.inverse[0].name(), "item.restore");
    assert_eq!(a.inverse[1], set("tr1", "c1", "c2", secs(1)));
    // The prune is in the op's footprint.
    let f = footprint(&delete("c2"), &a.diff);
    assert!(f.removes.contains(&"tr1".to_string()), "{f:?}");
}

/// A restore puts back what an edit took: the fit checks (kind, duration, handles) do not
/// apply to it, one per cut does.
#[test]
fn restores_skip_fit_checks() {
    let mut p = dissolved();
    // c2 lost its handle since the transition was set (a damaged or replayed history).
    if let ItemBody::Clip { source_in, .. } = &mut p.timelines[0].tracks[0].items[1].body {
        *source_in = Rational::zero();
    }
    let a = apply(&p, vec![delete("c1")]);
    assert!(cuts(&a.project).is_empty());
    assert_eq!(undo(&a), p);
    // A restore at a cut that has another transition is refused.
    let removed = apply(&dissolved(), vec![remove("tr1")]);
    // Ids keep counting past tr1, as the store's do.
    let other = fin_core::apply_tx(
        &removed.project,
        &tx(vec![set("$u", "c1", "c2", secs(1))]),
        &mut MemIds::after(&dissolved()),
    )
    .unwrap()
    .project;
    assert_eq!(cuts(&other), [cut("tr2", "c1", "c2")]);
    let e = fin_core::apply_internal(
        &other,
        &tx(removed.inverse.clone()),
        &mut MemIds::after(&other),
    )
    .unwrap_err();
    assert_eq!(e.code, "TRANSITION_EXISTS");
}

#[test]
fn pruned_transition_restored_by_undo() {
    let mut log = MemLog::new(clips());
    log.commit("main", "tx-1", "ann", vec![set("$t", "c1", "c2", secs(1))]);
    let with = log.state("main");
    log.commit("main", "tx-2", "ann", vec![delete("c1")]);
    assert!(cuts(&log.state("main")).is_empty());
    log.undo("main", "ann", "u-1").unwrap();
    assert_eq!(log.state("main"), with);
    log.redo("main", "ann", "r-1").unwrap();
    assert!(cuts(&log.state("main")).is_empty());
    log.undo("main", "ann", "u-2").unwrap();
    assert_eq!(log.state("main"), with);
}

#[test]
fn trim_separating_items_prunes() {
    let p = dissolved();
    for mode in [TrimMode::Overwrite, TrimMode::Strict] {
        let op: Op = ItemTrim {
            item: "c1".into(),
            edge: Edge::End,
            delta: secs(-1),
            mode,
            alone: false,
        }
        .into();
        let a = if mode == TrimMode::Strict {
            let a = fin_core::apply_internal(&p, &tx(vec![op]), &mut MemIds::after(&p)).unwrap();
            assert_eq!(undo(&a), p);
            a
        } else {
            golden_apply(&p, vec![op])
        };
        assert!(cuts(&a.project).is_empty(), "{mode:?}");
        assert_eq!(
            tr_changes(&a.diff),
            [("tr1".to_string(), ChangeKind::Removed)]
        );
    }
    // A start trim of c2 too.
    let a = golden_apply(
        &p,
        vec![
            ItemTrim {
                item: "c2".into(),
                edge: Edge::Start,
                delta: secs(1),
                mode: TrimMode::Overwrite,
                alone: false,
            }
            .into(),
        ],
    );
    assert!(cuts(&a.project).is_empty());
}

#[test]
fn move_prunes() {
    let p = dissolved();
    let a = golden_apply(&p, vec![move_to("c2", "t1", secs(12), InsertMode::Ripple)]);
    assert_eq!(
        spans_on(&a.project, 0),
        [sp("c1", 0, 4), sp("c3", 4, 4), sp("c2", 8, 4)]
    );
    assert!(cuts(&a.project).is_empty());
    // To another track: both tracks are touched.
    let q = with_v2(p.clone());
    let a = golden_apply(
        &q,
        vec![move_to("c1", "t3", secs(0), InsertMode::Overwrite)],
    );
    assert!(cuts(&a.project).is_empty());
    assert_eq!(
        tr_changes(&a.diff),
        [("tr1".to_string(), ChangeKind::Removed)]
    );
}

#[test]
fn overwrite_prunes() {
    let p = dissolved();
    let op: Op = ItemInsert {
        id: "$n".into(),
        track: "t1".into(),
        at: secs(3),
        duration: secs(2),
        mode: InsertMode::Overwrite,
        body: ItemBody::Clip {
            asset: "a1".into(),
            source_in: secs(50),
            speed: secs(1),
        },
    }
    .into();
    let a = golden_apply(&p, vec![op]);
    assert_eq!(
        spans_on(&a.project, 0),
        [
            sp("c1", 0, 3),
            sp("c4", 3, 2),
            sp("c2", 5, 3),
            sp("c3", 8, 4)
        ]
    );
    assert!(cuts(&a.project).is_empty());
}

#[test]
fn lift_prunes() {
    let p = dissolved();
    let op: Op = ItemDelete {
        item: "c2".into(),
        mode: DeleteMode::Lift,
    }
    .into();
    let a = golden_apply(&p, vec![op]);
    assert!(cuts(&a.project).is_empty());
    assert_eq!(a.inverse.len(), 2);
}

#[test]
fn ripple_elsewhere_keeps_transition() {
    let p = dissolved();
    let ops = vec![
        // A ripple insert at the next cut shifts c3; c1|c2 stays.
        insert("$n", "t1", secs(8), "a1", secs(0), secs(2)),
        // Ripple trims move the cut and its transition with it.
        trim("c1", Edge::End, secs(-1)),
        trim("c2", Edge::Start, secs(1)),
        delete("c3"),
        insert("$m", "t2", secs(0), "a1", secs(0), secs(3)),
        slip("c1"),
    ];
    for op in ops {
        let a = golden_apply(&p, vec![op.clone()]);
        assert_eq!(cuts(&a.project), [cut("tr1", "c1", "c2")], "{}", op.name());
        assert!(tr_changes(&a.diff).is_empty(), "{}", op.name());
    }
    // A roll moves the cut and keeps both items touching.
    let roll: Op = ItemTrim {
        item: "c1".into(),
        edge: Edge::End,
        delta: secs(1),
        mode: TrimMode::Roll,
        alone: false,
    }
    .into();
    let a = golden_apply(&p, vec![roll]);
    assert_eq!(cuts(&a.project), [cut("tr1", "c1", "c2")]);
}

fn slip(item: &str) -> Op {
    fin_core::ItemSlip {
        item: item.into(),
        delta: secs(1),
    }
    .into()
}

/// An inverse of several ops can pass through a state with the cut apart (a strict trim back,
/// then the carved neighbour's trim back): the transition is carried across it, so the undo
/// is exact.
#[test]
fn undo_of_overwrite_trim_keeps_transition() {
    let p = dissolved();
    let op: Op = ItemTrim {
        item: "c1".into(),
        edge: Edge::End,
        delta: secs(1),
        mode: TrimMode::Overwrite,
        alone: false,
    }
    .into();
    let a = golden_apply(&p, vec![op]);
    assert_eq!(
        spans_on(&a.project, 0),
        [sp("c1", 0, 5), sp("c2", 5, 3), sp("c3", 8, 4)]
    );
    assert_eq!(cuts(&a.project), [cut("tr1", "c1", "c2")]);
    assert_eq!(
        tr_changes(&a.diff),
        [("tr1".to_string(), ChangeKind::Updated)]
    );
    let names: Vec<&str> = a.inverse.iter().map(Op::name).collect();
    assert_eq!(
        names,
        [
            "transition.remove",
            "item.trim",
            "item.trim",
            "transition.set"
        ]
    );
    // Through the log too, redo included.
    let mut log = MemLog::new(p.clone());
    let op = ItemTrim {
        item: "c1".into(),
        edge: Edge::End,
        delta: secs(1),
        mode: TrimMode::Overwrite,
        alone: false,
    };
    log.commit("main", "tx-1", "ann", vec![op.into()]);
    let after = log.state("main");
    log.undo("main", "ann", "u-1").unwrap();
    assert_eq!(log.state("main"), p);
    log.redo("main", "ann", "r-1").unwrap();
    assert_eq!(log.state("main"), after);
}

#[test]
fn undo_of_overwrite_move_keeps_transition() {
    let p = dissolved();
    // c1 moves right by one, over c2's first second: they still touch.
    let a = golden_apply(
        &p,
        vec![move_to("c1", "t1", secs(1), InsertMode::Overwrite)],
    );
    assert_eq!(
        spans_on(&a.project, 0),
        [sp("c1", 1, 4), sp("c2", 5, 3), sp("c3", 8, 4)]
    );
    assert_eq!(cuts(&a.project), [cut("tr1", "c1", "c2")]);
    assert_eq!(
        tr_changes(&a.diff),
        [("tr1".to_string(), ChangeKind::Updated)]
    );
    // A ripple move to where the item already is keeps it too.
    let a = golden_apply(&p, vec![move_to("c2", "t1", secs(8), InsertMode::Ripple)]);
    assert_eq!(spans_on(&a.project, 0), spans_on(&p, 0));
    assert_eq!(cuts(&a.project), [cut("tr1", "c1", "c2")]);
}

/// A transition on an item whose op has a one-op inverse is never carried.
#[test]
fn one_op_inverses_carry_nothing() {
    let p = dissolved();
    for op in [trim("c1", Edge::End, secs(-1)), slide("c2", secs(1))] {
        let a = golden_apply(&p, vec![op.clone()]);
        assert_eq!(a.inverse.len(), 1, "{}", op.name());
        assert_eq!(cuts(&a.project), [cut("tr1", "c1", "c2")], "{}", op.name());
        assert!(tr_changes(&a.diff).is_empty(), "{}", op.name());
    }
}

// Rebase.

fn committed(p: &Project, ops: Vec<Op>, tx_id: &str) -> (Project, CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = tx_id.into();
    let a = fin_core::apply_tx(p, &t, &mut MemIds::after(p)).unwrap();
    let c = CommittedTx {
        rev: 1,
        tx: t,
        diff: a.diff,
        bindings: a.bindings,
        link: None,
    };
    (a.project, c)
}

fn rebase(
    p: &Project,
    theirs: Vec<Op>,
    ours: Vec<Op>,
) -> Result<fin_core::rebase::Rebased, CoreError> {
    let (head, c) = committed(p, theirs, "tx-a");
    let mut t: Tx = tx(ours);
    t.tx_id = "tx-b".into();
    rebase_tx(p, &head, &t, &[c], &mut MemIds::after(&head))
}

fn reason(e: &CoreError) -> &str {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    e.details["reason"].as_str().unwrap()
}

#[test]
fn two_sets_at_one_cut_conflict_same_param() {
    let p = clips();
    let e = rebase(
        &p,
        vec![set("$t", "c1", "c2", secs(1))],
        vec![set("$u", "c1", "c2", secs(2))],
    )
    .unwrap_err();
    assert_eq!(reason(&e), "SAME_PARAM");
    // Sets at different cuts merge.
    let r = rebase(
        &p,
        vec![set("$t", "c1", "c2", secs(1))],
        vec![set("$u", "c2", "c3", secs(1))],
    )
    .unwrap();
    assert_eq!(
        cuts(&r.applied.project),
        [cut("tr1", "c1", "c2"), cut("tr2", "c2", "c3")]
    );
}

#[test]
fn stale_edits_of_a_pruned_transition_conflict_removed() {
    let p = dissolved();
    for theirs in [delete("c2"), remove("tr1")] {
        let e = rebase(&p, vec![theirs], vec![set("tr1", "c1", "c2", secs(2))]).unwrap_err();
        assert_eq!(reason(&e), "REMOVED");
        assert_eq!(e.details["againstTx"], "tx-a");
    }
}

#[test]
fn stale_ripple_insert_keeps_the_transition() {
    let p = dissolved();
    let r = rebase(
        &p,
        vec![set("tr1", "c1", "c2", secs(2))],
        vec![insert("$n", "t1", secs(0), "a1", secs(0), secs(1))],
    )
    .unwrap();
    assert_eq!(cuts(&r.applied.project), [cut("tr1", "c1", "c2")]);
    assert_eq!(the_transition(&r.applied.project).duration, secs(2));
}

// Property: with a transition at every cut, any structural edit is undone exactly (pruned
// transitions restored, carried ones kept), and the projects it leaves hold valid transitions.

mod props {
    use super::*;
    use fin_core::{ItemMove, ItemSlide, ItemSplit};
    use proptest::prelude::*;

    /// Four 6 s clips with source on both sides of every cut, and a 1 s dissolve at each cut.
    fn genesis() -> Project {
        let mut p = with_sourced(&[(6, 10), (6, 20), (6, 30), (6, 40)]);
        let cuts: Vec<Op> = (1..4)
            .map(|i| {
                set(
                    &format!("$t{i}"),
                    &format!("c{i}"),
                    &format!("c{}", i + 1),
                    secs(1),
                )
            })
            .collect();
        p = apply(&p, cuts).project;
        assert_eq!(p.timelines[0].tracks[0].transitions.len(), 3);
        p
    }

    fn halves(n: i64) -> Rational {
        Rational::new(n, 2).unwrap()
    }

    #[derive(Debug, Clone)]
    enum G {
        Trim(usize, bool, i64, u8),
        Move(usize, i64, bool),
        Delete(usize, bool),
        Split(usize),
        Overwrite(usize, i64),
        Slide(usize, i64),
        Set(usize),
        Remove(usize),
    }

    fn g() -> impl Strategy<Value = G> {
        let d = prop_oneof![-4i64..-0, 1i64..5];
        prop_oneof![
            3 => (0usize..5, any::<bool>(), d.clone(), 0u8..3).prop_map(|(k, s, d, m)| G::Trim(k, s, d, m)),
            2 => (0usize..5, -6i64..14, any::<bool>()).prop_map(|(k, d, o)| G::Move(k, d, o)),
            1 => (0usize..5, any::<bool>()).prop_map(|(k, r)| G::Delete(k, r)),
            1 => (0usize..5).prop_map(G::Split),
            1 => (0usize..5, -4i64..14).prop_map(|(k, d)| G::Overwrite(k, d)),
            1 => (0usize..5, d).prop_map(|(k, d)| G::Slide(k, d)),
            1 => (0usize..5).prop_map(G::Set),
            1 => (0usize..5).prop_map(G::Remove),
        ]
    }

    fn op_of(p: &Project, g: &G) -> Option<Op> {
        let items = &p.timelines[0].tracks[0].items;
        let it = |k: usize| items.get(k % items.len().max(1));
        Some(match *g {
            G::Trim(k, start, d, m) => ItemTrim {
                item: it(k)?.id.clone(),
                edge: if start { Edge::Start } else { Edge::End },
                delta: halves(d),
                mode: [TrimMode::Ripple, TrimMode::Roll, TrimMode::Overwrite][m as usize],
                alone: false,
            }
            .into(),
            G::Move(k, d, overwrite) => ItemMove {
                item: it(k)?.id.clone(),
                track: "t1".into(),
                at: (it(k)?.start + halves(d)).ok()?,
                mode: if overwrite {
                    InsertMode::Overwrite
                } else {
                    InsertMode::Ripple
                },
            }
            .into(),
            G::Delete(k, ripple) => ItemDelete {
                item: it(k)?.id.clone(),
                mode: if ripple {
                    DeleteMode::Ripple
                } else {
                    DeleteMode::Lift
                },
            }
            .into(),
            G::Split(k) => ItemSplit {
                item: it(k)?.id.clone(),
                offset: it(k)?.duration.checked_div(secs(2)).ok()?,
                id: "$r".into(),
            }
            .into(),
            G::Overwrite(k, d) => ItemInsert {
                id: "$o".into(),
                track: "t1".into(),
                at: (it(k)?.start + halves(d)).ok()?,
                duration: secs(2),
                mode: InsertMode::Overwrite,
                body: ItemBody::Clip {
                    asset: "a1".into(),
                    source_in: secs(50),
                    speed: secs(1),
                },
            }
            .into(),
            G::Slide(k, d) => ItemSlide {
                item: it(k)?.id.clone(),
                delta: halves(d),
                sides: None,
            }
            .into(),
            G::Set(k) => set("$t", &it(k)?.id, &it(k + 1)?.id, r("1/2")),
            G::Remove(k) => {
                let ts = &p.timelines[0].tracks[0].transitions;
                remove(&ts.get(k % ts.len().max(1))?.id)
            }
        })
    }

    proptest! {
        #![proptest_config(prop_config(1500))]

        #[test]
        fn structural_edits_undo_exactly_with_transitions(gs in prop::collection::vec(g(), 1..6)) {
            let mut p = genesis();
            for g in &gs {
                let Some(op) = op_of(&p, g) else { continue };
                let Ok(a) = try_apply(&p, vec![op.clone()]) else { continue };
                prop_assert_eq!(&undo(&a), &p, "{:?}", op);
                // What is left is valid: each transition at a cut, one per cut.
                assert_well_formed(&a.project);
                for t in &a.project.timelines[0].tracks[0].transitions {
                    let items = &a.project.timelines[0].tracks[0].items;
                    let i = items.iter().position(|c| c.id == t.between[0]).unwrap();
                    prop_assert_eq!(&items[i + 1].id, &t.between[1]);
                    prop_assert_eq!((items[i].start + items[i].duration).unwrap(), items[i + 1].start);
                }
                p = a.project;
            }
        }
    }
}
