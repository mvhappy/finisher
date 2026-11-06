//! Overwrite, roll, slip and split (plan 4C Task 8): goldens of apply, inverse and shifts for
//! every mode, with `inverse_shifts_mirror` (Decision 23); carving; the roll neighbour; the slip
//! source range as policy; split copies; `compose::overwrite`; footprints and ids (B6).

mod common;

use common::ids_json::ids_cover_every_id_in_json;
use common::*;
use fin_core::compose;
use fin_core::rebase::{Touch, footprint, split_diff};
use fin_core::{
    Applied, ApplyMode, CaptionBlock, Change, ChangeKind, DeleteMode, Edge, Effect, EntityKind,
    InsertMode, Interp, Item, ItemBody, ItemInsert, ItemRemove, ItemRestore, ItemSlip, ItemSplit,
    ItemTrim, Keyframe, MemIds, Op, ParamValue, Project, Rational, Track, TrackKind, TrimMode,
    Word, apply_tx_mode,
};
use serde_json::json;

fn clip_body(source_in: Rational) -> ItemBody {
    ItemBody::Clip {
        asset: "a1".into(),
        source_in,
        speed: secs(1),
    }
}

fn overwrite(id: &str, at: Rational, duration: Rational, source_in: Rational) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t1".into(),
        at,
        duration,
        mode: InsertMode::Overwrite,
        body: clip_body(source_in),
    }
    .into()
}

fn trim_in(item: &str, edge: Edge, delta: Rational, mode: TrimMode) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode,
    }
    .into()
}

fn slip(item: &str, delta: Rational) -> Op {
    ItemSlip {
        item: item.into(),
        delta,
    }
    .into()
}

fn split(item: &str, offset: Rational, id: &str) -> Op {
    ItemSplit {
        item: item.into(),
        offset,
        id: id.into(),
    }
    .into()
}

fn change(id: &str, change: ChangeKind, edge: Option<Edge>, by: Option<Rational>) -> Change {
    Change {
        kind: EntityKind::Item,
        id: id.into(),
        change,
        op: 0,
        edge,
        path: None,
        by,
    }
}

fn spans(p: &Project) -> Vec<(String, Rational, Rational)> {
    p.timelines[0].tracks[0]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start, c.duration))
        .collect()
}

fn item<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timelines[0]
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no item {id}"))
}

fn item_mut<'a>(p: &'a mut Project, id: &str) -> &'a mut Item {
    p.timelines[0]
        .tracks
        .iter_mut()
        .flat_map(|t| &mut t.items)
        .find(|c| c.id == id)
        .unwrap()
}

fn keyframes(at: &[i64]) -> Vec<Keyframe> {
    at.iter()
        .map(|&a| Keyframe {
            at: secs(a),
            value: ParamValue::Number(0.5),
            interp: Interp::Linear,
        })
        .collect()
}

fn err(p: &Project, ops: Vec<Op>) -> fin_core::CoreError {
    try_apply(p, ops).expect_err("expected an error")
}

fn replay(p: &Project, ops: Vec<Op>) -> Result<Applied, fin_core::CoreError> {
    apply_tx_mode(p, &tx(ops), &mut MemIds::after(p), ApplyMode::Replay)
}

fn s(id: &str, start: i64, duration: i64) -> (String, Rational, Rational) {
    (id.to_string(), secs(start), secs(duration))
}

// ---- overwrite insert ----

#[test]
fn overwrite_insert_golden() {
    let p = with_clips(&[10, 10, 10]);
    let a = golden_apply(&p, vec![overwrite("$x", secs(5), secs(20), secs(50))]);
    assert_eq!(a.bindings["$x"], "c4");
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 5), s("c4", 5, 20), s("c3", 25, 5)]
    );
    assert_eq!(clip(&a.project, "c3").source_in, secs(5));
    assert!(a.diff.shifts.is_empty(), "an overwrite records no shifts");
    assert_eq!(
        a.diff.changes,
        [
            change("c1", ChangeKind::Updated, Some(Edge::End), None),
            change("c2", ChangeKind::Removed, None, None),
            change("c3", ChangeKind::Updated, Some(Edge::Start), Some(secs(-5))),
            change("c4", ChangeKind::Added, None, None),
        ]
    );
    let c2 = item(&p, "c2").clone();
    assert_eq!(
        a.inverse,
        [
            ItemRemove {
                id: "c4".into(),
                mode: DeleteMode::Lift
            }
            .into(),
            trim_in("c1", Edge::End, secs(5), TrimMode::Strict),
            trim_in("c3", Edge::Start, secs(-5), TrimMode::Strict),
            ItemRestore {
                item: c2,
                track: "t1".into(),
                mode: DeleteMode::Lift
            }
            .into(),
        ]
    );
    assert_eq!(
        serde_json::to_value(overwrite("$x", secs(1), secs(2), secs(0))).unwrap()["mode"],
        json!("overwrite")
    );
    assert_well_formed(&a.project);
}

#[test]
fn overwrite_removes_covered_and_trims_partial() {
    let p = with_clips(&[10, 10, 10]);
    // Exactly one item: removed, its neighbours untouched.
    let a = golden_apply(&p, vec![overwrite("$x", secs(10), secs(10), secs(0))]);
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 10), s("c4", 10, 10), s("c3", 20, 10)]
    );
    assert_eq!(a.diff.changes.len(), 2);
    // From a boundary into the next item: only that one is trimmed, at its start.
    let a = golden_apply(&p, vec![overwrite("$x", secs(10), secs(3), secs(0))]);
    assert_eq!(
        spans(&a.project),
        [
            s("c1", 0, 10),
            s("c4", 10, 3),
            s("c2", 13, 7),
            s("c3", 20, 10)
        ]
    );
    // Past the end, in a gap: nothing carved.
    let a = golden_apply(&p, vec![overwrite("$x", secs(40), secs(3), secs(0))]);
    assert_eq!(
        a.diff.changes,
        [change("c4", ChangeKind::Added, None, None)]
    );
    // Covering everything.
    let a = golden_apply(&p, vec![overwrite("$x", secs(0), secs(30), secs(0))]);
    assert_eq!(spans(&a.project), [s("c4", 0, 30)]);
    assert_eq!(a.inverse.len(), 4);
    // The start-trimmed item keeps its content where it was on the timeline.
    let mut q = with_clips(&[10, 10]);
    item_mut(&mut q, "c2")
        .common
        .keyframes
        .insert("opacity".into(), keyframes(&[0, 4]));
    let a = golden_apply(&q, vec![overwrite("$x", secs(8), secs(4), secs(0))]);
    assert_eq!(
        item(&a.project, "c2").common.keyframes["opacity"],
        keyframes(&[-2, 2])
    );
}

#[test]
fn overwrite_inside_one_item_is_refused() {
    let p = with_clips(&[10]);
    for mode in [ApplyMode::Client, ApplyMode::Replay] {
        let e = apply_tx_mode(
            &p,
            &tx(vec![overwrite("$x", secs(2), secs(3), secs(0))]),
            &mut MemIds::after(&p),
            mode,
        )
        .unwrap_err();
        assert_eq!(e.code, "OVERWRITE_SPLITS_ITEM", "{mode:?}");
        assert_eq!(e.details["item"], "c1");
        assert!(e.fix.unwrap().contains("split"));
    }
    // Policy still refuses a negative position; a locked track refuses it.
    assert_eq!(
        err(&p, vec![overwrite("$x", secs(-1), secs(3), secs(0))]).code,
        "INVALID_RANGE"
    );
    let mut locked = p.clone();
    locked.timelines[0].tracks[0].lock = true;
    assert_eq!(
        err(&locked, vec![overwrite("$x", secs(10), secs(3), secs(0))]).code,
        "TRACK_LOCKED"
    );
}

#[test]
fn compose_overwrite_splits_first() {
    let p = with_clips(&[10]);
    let c = compose::overwrite(&p, "t1", secs(2), secs(3), clip_body(secs(50)), "$x").unwrap();
    assert_eq!(
        c.ops,
        [
            split("c1", secs(2), "$split"),
            overwrite("$x", secs(2), secs(3), secs(50))
        ]
    );
    assert_eq!(
        c.echo,
        json!({"mode": "overwrite", "split": {"item": "c1", "offset": "2", "id": "$split"}})
    );
    let a = golden_apply(&p, c.ops);
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 2), s("c3", 2, 3), s("c2", 5, 5)]
    );
    assert_eq!(clip(&a.project, "c2").source_in, secs(5));
    assert_well_formed(&a.project);
    // Not inside one item: the insert alone.
    let c = compose::overwrite(&p, "t1", secs(5), secs(10), clip_body(secs(0)), "$x").unwrap();
    assert_eq!(c.ops, [overwrite("$x", secs(5), secs(10), secs(0))]);
    assert_eq!(c.echo["split"], json!(null));
    // The split placeholder never collides with the insert's.
    let c = compose::overwrite(&p, "t1", secs(2), secs(3), clip_body(secs(0)), "$split").unwrap();
    golden_apply(&p, c.ops);
    assert_eq!(
        compose::overwrite(&p, "t9", secs(2), secs(3), clip_body(secs(0)), "$x")
            .unwrap_err()
            .code,
        "NOT_FOUND"
    );
}

// ---- overwrite trim ----

#[test]
fn overwrite_trim_golden() {
    let p = with_sourced(&[(10, 10), (10, 10), (10, 10)]);
    // End edge out: carves the next item's start.
    let a = golden_apply(
        &p,
        vec![trim_in("c1", Edge::End, secs(5), TrimMode::Overwrite)],
    );
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 15), s("c2", 15, 5), s("c3", 20, 10)]
    );
    assert_eq!(clip(&a.project, "c2").source_in, secs(15));
    assert!(a.diff.shifts.is_empty());
    assert_eq!(
        a.diff.changes,
        [
            change("c2", ChangeKind::Updated, Some(Edge::Start), Some(secs(-5))),
            change("c1", ChangeKind::Updated, Some(Edge::End), None),
        ]
    );
    assert_eq!(
        a.inverse,
        [
            trim_in("c1", Edge::End, secs(-5), TrimMode::Strict),
            trim_in("c2", Edge::Start, secs(-5), TrimMode::Strict),
        ]
    );
    // Start edge out over a whole item and part of another.
    let a = golden_apply(
        &p,
        vec![trim_in("c3", Edge::Start, secs(-10), TrimMode::Overwrite)],
    );
    assert_eq!(spans(&a.project), [s("c1", 0, 10), s("c3", 10, 20)]);
    assert_eq!(clip(&a.project, "c3").source_in, secs(0));
    assert_eq!(a.diff.changes[0].change, ChangeKind::Removed);
    assert_eq!(a.inverse.len(), 2);
    // Shrinking leaves a gap and moves nothing.
    let a = golden_apply(
        &p,
        vec![trim_in("c2", Edge::End, secs(-3), TrimMode::Overwrite)],
    );
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 10), s("c2", 10, 7), s("c3", 20, 10)]
    );
    let a = golden_apply(
        &p,
        vec![trim_in("c2", Edge::Start, secs(3), TrimMode::Overwrite)],
    );
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 10), s("c2", 13, 7), s("c3", 20, 10)]
    );
    assert_eq!(a.diff.changes[0].by, Some(secs(-3)));
    // The source range is policy, as for any trim; a start edge stays at 0 or later.
    assert_eq!(
        err(
            &p,
            vec![trim_in("c1", Edge::Start, secs(-1), TrimMode::Overwrite)]
        )
        .code,
        "INVALID_RANGE"
    );
    assert_eq!(
        serde_json::to_value(trim_in("c1", Edge::End, secs(1), TrimMode::Roll)).unwrap()["mode"],
        json!("roll")
    );
}

// ---- roll ----

#[test]
fn roll_moves_both_edges_and_source() {
    let mut p = with_sourced(&[(10, 10), (10, 10), (10, 10)]);
    item_mut(&mut p, "c2")
        .common
        .keyframes
        .insert("opacity".into(), keyframes(&[1]));
    let a = golden_apply(&p, vec![trim_in("c1", Edge::End, secs(2), TrimMode::Roll)]);
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 12), s("c2", 12, 8), s("c3", 20, 10)]
    );
    assert_eq!(clip(&a.project, "c2").source_in, secs(12));
    assert_eq!(
        item(&a.project, "c2").common.keyframes["opacity"],
        keyframes(&[-1])
    );
    assert!(a.diff.shifts.is_empty());
    assert_eq!(
        a.diff.changes,
        [
            change("c1", ChangeKind::Updated, Some(Edge::End), None),
            change("c2", ChangeKind::Updated, Some(Edge::Start), Some(secs(-2))),
        ]
    );
    assert_eq!(
        a.inverse,
        [trim_in("c1", Edge::End, secs(-2), TrimMode::Roll)]
    );
    // The start edge of the named item: it records `by`, its neighbour the end edge.
    let a = golden_apply(
        &p,
        vec![trim_in("c2", Edge::Start, secs(-3), TrimMode::Roll)],
    );
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 7), s("c2", 7, 13), s("c3", 20, 10)]
    );
    assert_eq!(clip(&a.project, "c2").source_in, secs(7));
    assert_eq!(
        a.diff.changes,
        [
            change("c2", ChangeKind::Updated, Some(Edge::Start), Some(secs(3))),
            change("c1", ChangeKind::Updated, Some(Edge::End), None),
        ]
    );
    // Either item's limits bound the roll (policy).
    let e = err(&p, vec![trim_in("c1", Edge::End, secs(10), TrimMode::Roll)]);
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(e.details["max"], json!(r("299/30")));
    assert_eq!(
        err(
            &p,
            vec![trim_in("c2", Edge::Start, secs(-11), TrimMode::Roll)]
        )
        .code,
        "INVALID_RANGE"
    );
}

#[test]
fn roll_at_speed_moves_the_source_by_speed() {
    let p = with_sourced(&[(10, 0)]);
    let ops = vec![
        ItemInsert {
            id: "$f".into(),
            track: "t1".into(),
            at: secs(10),
            duration: secs(10),
            mode: InsertMode::Ripple,
            body: ItemBody::Clip {
                asset: "a1".into(),
                source_in: secs(20),
                speed: secs(2),
            },
        }
        .into(),
    ];
    let p = apply(&p, ops).project;
    let a = golden_apply(&p, vec![trim_in("c1", Edge::End, secs(3), TrimMode::Roll)]);
    assert_eq!(clip(&a.project, "c2").source_in, secs(26));
    assert_eq!(clip(&a.project, "c2").duration, secs(7));
}

#[test]
fn roll_requires_adjacent_neighbour() {
    let p = with_clips(&[10, 10]);
    for (op, base) in [
        (trim_in("c2", Edge::End, secs(1), TrimMode::Roll), p.clone()),
        (
            trim_in("c1", Edge::Start, secs(1), TrimMode::Roll),
            p.clone(),
        ),
        (
            trim_in("c1", Edge::End, secs(1), TrimMode::Roll),
            apply(
                &p,
                vec![trim_in("c1", Edge::End, secs(-2), TrimMode::Overwrite)],
            )
            .project,
        ),
    ] {
        for mode in [ApplyMode::Client, ApplyMode::Replay] {
            let e = apply_tx_mode(
                &base,
                &tx(vec![op.clone()]),
                &mut MemIds::after(&base),
                mode,
            )
            .unwrap_err();
            assert_eq!(e.code, "NOT_ADJACENT", "{op:?} {mode:?}");
        }
    }
}

// ---- slip ----

#[test]
fn slip_keeps_timeline_positions() {
    let mut p = with_clips(&[10, 10]);
    item_mut(&mut p, "c2")
        .common
        .keyframes
        .insert("opacity".into(), keyframes(&[2]));
    let a = golden_apply(&p, vec![slip("c2", secs(3))]);
    assert_eq!(spans(&a.project), spans(&p));
    assert_eq!(clip(&a.project, "c2").source_in, secs(3));
    assert_eq!(
        item(&a.project, "c2").common.keyframes["opacity"],
        keyframes(&[2])
    );
    assert!(a.diff.shifts.is_empty());
    assert_eq!(
        a.diff.changes,
        [Change {
            path: Some("sourceIn".into()),
            ..change("c2", ChangeKind::Updated, None, None)
        }]
    );
    assert_eq!(a.inverse, [slip("c2", secs(-3))]);
    assert_eq!(
        serde_json::to_value(slip("c2", r("1/2"))).unwrap(),
        json!({"op": "item.slip", "item": "c2", "delta": "1/2"})
    );
    // Only clips and nested items have a source.
    let q = apply(
        &project(),
        vec![
            ItemInsert {
                id: "$j".into(),
                track: "t1".into(),
                at: secs(0),
                duration: secs(2),
                mode: InsertMode::Ripple,
                body: ItemBody::Adjustment {},
            }
            .into(),
        ],
    )
    .project;
    assert_eq!(err(&q, vec![slip("adj1", secs(1))]).code, "INVALID_OP");
}

#[test]
fn slip_source_range_is_policy() {
    let p = with_clips(&[10]);
    let e = err(&p, vec![slip("c1", secs(91))]);
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(
        (e.details["min"].clone(), e.details["max"].clone()),
        (json!("0"), json!("90"))
    );
    assert_eq!(err(&p, vec![slip("c1", secs(-1))]).code, "INVALID_RANGE");
    // Replay applies a slip past the asset end; the track check still refuses a negative
    // source.
    let a = replay(&p, vec![slip("c1", secs(95))]).unwrap();
    assert_eq!(clip(&a.project, "c1").source_in, secs(95));
    assert_eq!(
        replay(&p, vec![slip("c1", secs(-1))]).unwrap_err().code,
        "CLIP_NEGATIVE"
    );
    let mut locked = p.clone();
    locked.timelines[0].tracks[0].lock = true;
    assert_eq!(err(&locked, vec![slip("c1", secs(1))]).code, "TRACK_LOCKED");
}

// ---- split ----

#[test]
fn split_copies_effects_keyframes_rebased() {
    let mut p = with_sourced(&[(10, 10), (10, 0)]);
    let c1 = item_mut(&mut p, "c1");
    c1.common.effects.push(Effect {
        id: "fx9".into(),
        kind: "blur".into(),
        enabled: true,
        params: [("radius".to_string(), ParamValue::Number(2.0))].into(),
    });
    c1.common
        .keyframes
        .insert("opacity".into(), keyframes(&[0, 7]));
    c1.common.link = Some("lk1".into());
    let a = golden_apply(&p, vec![split("c1", secs(4), "$r")]);
    assert_eq!(a.bindings["$r"], "c3");
    assert_eq!(
        spans(&a.project),
        [s("c1", 0, 4), s("c3", 4, 6), s("c2", 10, 10)]
    );
    let (left, right) = (item(&a.project, "c1"), item(&a.project, "c3"));
    assert_eq!(clip(&a.project, "c3").source_in, secs(14));
    assert_eq!(
        right.common.effects, left.common.effects,
        "effects keep their ids"
    );
    assert_eq!(
        left.common.keyframes["opacity"],
        keyframes(&[0, 7]),
        "left keeps its times"
    );
    assert_eq!(right.common.keyframes["opacity"], keyframes(&[-4, 3]));
    assert_eq!(
        (left.common.link.as_deref(), right.common.link.as_deref()),
        (Some("lk1"), None)
    );
    assert!(a.diff.shifts.is_empty());
    assert_eq!(
        a.diff.changes,
        [
            change("c1", ChangeKind::Updated, None, None),
            change("c3", ChangeKind::Added, None, None),
        ]
    );
    assert_eq!(
        a.inverse,
        [
            ItemRemove {
                id: "c3".into(),
                mode: DeleteMode::Lift
            }
            .into(),
            trim_in("c1", Edge::End, secs(6), TrimMode::Strict),
        ]
    );
    assert_eq!(
        serde_json::to_value(split("c1", r("1/2"), "$r")).unwrap(),
        json!({"op": "item.split", "item": "c1", "offset": "1/2", "id": "$r"})
    );
    assert_well_formed(&a.project);
}

#[test]
fn split_on_caption_is_invalid_op_with_fix() {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    let body = ItemBody::Caption {
        block: CaptionBlock {
            words: vec![Word {
                text: "hi".into(),
                start: secs(0),
                end: secs(1),
            }],
            style: "preset:bold".into(),
            overrides: Default::default(),
            source: None,
        },
    };
    let ins = ItemInsert {
        id: "$k".into(),
        track: "t3".into(),
        at: secs(0),
        duration: secs(4),
        mode: InsertMode::Ripple,
        body,
    };
    let p = apply(&p, vec![ins.into()]).project;
    for mode in [ApplyMode::Client, ApplyMode::Replay] {
        let e = apply_tx_mode(
            &p,
            &tx(vec![split("cap1", secs(2), "cap9")]),
            &mut MemIds::after(&p),
            mode,
        );
        let e = e.unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.fix.as_deref()),
            ("INVALID_OP", Some("use caption.split"))
        );
    }
}

#[test]
fn split_offset_rules() {
    let p = with_clips(&[10]);
    for offset in [secs(0), secs(10), secs(-1), secs(11)] {
        assert_eq!(
            replay(&p, vec![split("c1", offset, "c9")])
                .unwrap_err()
                .code,
            "INVALID_RANGE",
            "{offset} is structural"
        );
    }
    // Under a frame from an end is policy.
    assert_eq!(
        err(&p, vec![split("c1", r("1/60"), "$r")]).code,
        "INVALID_RANGE"
    );
    assert_eq!(
        err(&p, vec![split("c1", r("599/60"), "$r")]).code,
        "INVALID_RANGE"
    );
    let a = replay(&p, vec![split("c1", r("1/60"), "c9")]).unwrap();
    assert_eq!(item(&a.project, "c9").duration, r("599/60"));
    // A literal right-part id is internal.
    assert_eq!(err(&p, vec![split("c1", secs(5), "c9")]).code, "INVALID_OP");
}

// ---- footprints and ids ----

fn own_footprint(p: &Project, op: Op) -> fin_core::rebase::Footprint {
    let a = apply(p, vec![op.clone()]);
    let d = split_diff(std::slice::from_ref(&op), &a.diff).unwrap();
    footprint(&op, &d[0])
}

#[test]
fn footprints_come_from_each_ops_diff() {
    let p = with_sourced(&[(10, 10), (10, 10), (10, 10)]);
    let f = own_footprint(&p, overwrite("$x", secs(5), secs(20), secs(0)));
    assert!(f.creates.contains(&"$x".to_string()));
    assert_eq!(f.removes, ["c2"]);
    assert_eq!(
        f.touches,
        [
            ("c1".to_string(), Touch::Edge(Edge::End)),
            ("c3".to_string(), Touch::Edge(Edge::Start))
        ]
    );
    assert!(f.tracks.contains("t1"));
    let f = own_footprint(&p, trim_in("c1", Edge::End, secs(2), TrimMode::Roll));
    assert_eq!(
        f.touches,
        [
            ("c1".to_string(), Touch::Edge(Edge::End)),
            ("c2".to_string(), Touch::Edge(Edge::Start))
        ]
    );
    let f = own_footprint(&p, slip("c2", secs(1)));
    assert_eq!(
        f.touches,
        [("c2".to_string(), Touch::Param("sourceIn".into()))]
    );
    let f = own_footprint(&p, split("c2", secs(5), "$r"));
    assert_eq!(f.touches, [("c2".to_string(), Touch::Exists)]);
    assert!(f.creates.contains(&"$r".to_string()));
}

#[test]
fn new_ops_list_their_ids() {
    for op in [slip("c1", secs(1)), split("c1", secs(1), "$r")] {
        ids_cover_every_id_in_json(&op);
    }
    assert_eq!(split("c1", secs(1), "$r").ids(), ["c1", "$r"]);
    assert_eq!(split("c1", secs(1), "$r").allocates(), Some("$r"));
    // Every carve inverse lists the ids nested in what it restores (B6).
    let mut p = with_clips(&[10, 10, 10]);
    item_mut(&mut p, "c2").common.effects.push(Effect {
        id: "fx4".into(),
        kind: "blur".into(),
        enabled: true,
        params: Default::default(),
    });
    let a = apply(&p, vec![overwrite("$x", secs(5), secs(20), secs(0))]);
    for op in &a.inverse {
        ids_cover_every_id_in_json(op);
    }
    assert!(a.inverse[3].ids().contains(&"fx4"));
}

#[test]
fn strict_is_still_internal() {
    let p = with_clips(&[10, 10]);
    assert_eq!(
        err(
            &p,
            vec![trim_in("c1", Edge::End, secs(-1), TrimMode::Strict)]
        )
        .code,
        "INVALID_OP"
    );
    // Client modes are accepted.
    for mode in [TrimMode::Ripple, TrimMode::Roll, TrimMode::Overwrite] {
        golden_apply(&p, vec![trim_in("c1", Edge::End, secs(1), mode)]);
    }
}
