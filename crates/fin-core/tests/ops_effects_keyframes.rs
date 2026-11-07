//! Effects and keyframes (plan 4C Task 14): goldens of `effect.add|remove|restore|move` and
//! `keyframe.set|remove` (exact inverses, `ids_cover_every_id_in_json`), effect ids from the
//! project-wide counter (kept by a split), keyframe identity across start trims (B3, content
//! offsets), split behaviour, and conflicts by parameter (Decision 20).

mod common;

use std::collections::BTreeMap;

use common::ids_json::ids_cover_every_id_in_json;
use common::memlog::MemLog;
use common::*;
use fin_core::params;
use fin_core::{
    ApplyMode, AssetAdd, AssetKind, ChangeKind, CommittedTx, CoreError, Edge, Effect, EffectAdd,
    EffectMove, EffectRemove, EffectRestore, EntityKind, Interp, Item, ItemSplit, Keyframe,
    KeyframeRemove, KeyframeSet, LutInfo, MemIds, Op, ParamSet, ParamValue, Project, Rational,
    apply_tx, apply_tx_mode, rebase_tx,
};
use serde_json::json;

fn add(item: &str, id: &str, kind: &str, at: Option<u32>) -> Op {
    EffectAdd {
        item: item.into(),
        id: id.into(),
        kind: kind.into(),
        at,
        params: BTreeMap::new(),
    }
    .into()
}

fn remove(item: &str, effect: &str) -> Op {
    EffectRemove {
        item: item.into(),
        effect: effect.into(),
    }
    .into()
}

fn mv(item: &str, effect: &str, to: u32) -> Op {
    EffectMove {
        item: item.into(),
        effect: effect.into(),
        to,
    }
    .into()
}

fn key(item: &str, path: &str, at: Rational, v: f64) -> Op {
    KeyframeSet {
        item: item.into(),
        path: path.into(),
        at,
        value: ParamValue::Number(v),
        interp: Interp::Linear,
    }
    .into()
}

fn unkey(item: &str, path: &str, at: Rational) -> Op {
    KeyframeRemove {
        item: item.into(),
        path: path.into(),
        at,
    }
    .into()
}

fn set(item: &str, path: &str, v: f64) -> Op {
    ParamSet {
        item: item.into(),
        path: path.into(),
        value: Some(ParamValue::Number(v)),
    }
    .into()
}

fn item<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timelines
        .iter()
        .flat_map(|t| &t.tracks)
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}"))
}

fn effect_ids(p: &Project, id: &str) -> Vec<String> {
    item(p, id)
        .common
        .effects
        .iter()
        .map(|e| e.id.clone())
        .collect()
}

fn keys(p: &Project, id: &str, path: &str) -> Vec<(Rational, ParamValue)> {
    item(p, id)
        .common
        .keyframes
        .get(path)
        .map(|k| k.iter().map(|k| (k.at, k.value.clone())).collect())
        .unwrap_or_default()
}

fn lut_asset(id: &str) -> Op {
    let mut a = AssetAdd::media(id, "/m/look.cube", secs(1), None, None, "fp");
    a.kind = AssetKind::Lut;
    a.duration = None;
    a.lut = Some(LutInfo { size: 33 });
    a.into()
}

/// `c1` (10 s from source 10) and `c2` (10 s) on `t1`; `c1` has `fx1` blur and `fx2` lut (on
/// `a2`), with opacity keyframes at 2 and 6 and radius keyframes on `fx1` at 1 and 4.
fn busy() -> Project {
    let p = with_sourced(&[(10, 10), (10, 0)]);
    let mut lut = BTreeMap::new();
    lut.insert(
        "asset".to_string(),
        ParamValue::Asset { asset: "$l".into() },
    );
    apply(
        &p,
        vec![
            lut_asset("$l"),
            add("c1", "$b", "blur", None),
            EffectAdd {
                item: "c1".into(),
                id: "$t".into(),
                kind: "lut".into(),
                at: None,
                params: lut,
            }
            .into(),
            key("c1", "opacity", secs(2), 1.0),
            key("c1", "opacity", secs(6), 0.0),
            key("c1", "effects.fx1.radius", secs(1), 0.0),
            key("c1", "effects.fx1.radius", secs(4), 20.0),
        ],
    )
    .project
}

// ---- goldens ----

#[test]
fn effect_ops_golden() {
    let p = busy();
    assert_eq!(effect_ids(&p, "c1"), ["fx1", "fx2"]);
    // Add, appended or at an index, with params.
    let a = golden_apply(&p, vec![add("c1", "$fx", "colorAdjust", None)]);
    assert_eq!(effect_ids(&a.project, "c1"), ["fx1", "fx2", "fx3"]);
    assert_eq!(a.bindings["$fx"], "fx3");
    let [c] = &a.diff.changes[..] else {
        panic!("{:?}", a.diff)
    };
    assert_eq!(
        (c.kind, c.id.as_str(), c.change, c.track.as_deref()),
        (EntityKind::Effect, "c1/fx3", ChangeKind::Added, Some("t1"))
    );
    let mut params = BTreeMap::new();
    params.insert("exposure".to_string(), ParamValue::Number(-0.0));
    let op: Op = EffectAdd {
        item: "c1".into(),
        id: "$fx".into(),
        kind: "colorAdjust".into(),
        at: Some(0),
        params,
    }
    .into();
    ids_cover_every_id_in_json(&op);
    let a = golden_apply(&p, vec![op]);
    assert_eq!(effect_ids(&a.project, "c1"), ["fx3", "fx1", "fx2"]);
    let e = &item(&a.project, "c1").common.effects[0];
    assert!(matches!(e.params["exposure"], ParamValue::Number(x) if x.to_bits() == 0));
    // With an index the effect order is touched too.
    assert_eq!(a.diff.changes[1].path.as_deref(), Some("effects"));
    // Remove takes the effect's keyframes along; its restore puts both back.
    let a = golden_apply(&p, vec![remove("c1", "fx1")]);
    assert_eq!(effect_ids(&a.project, "c1"), ["fx2"]);
    assert!(keys(&a.project, "c1", "effects.fx1.radius").is_empty());
    assert_eq!(keys(&a.project, "c1", "opacity").len(), 2);
    let Op::EffectRestore(r) = &a.inverse[0] else {
        panic!("{:?}", a.inverse)
    };
    assert_eq!((r.index, r.effect.id.as_str()), (0, "fx1"));
    assert_eq!(
        r.keyframes.keys().collect::<Vec<_>>(),
        ["effects.fx1.radius"]
    );
    ids_cover_every_id_in_json(&a.inverse[0]);
    let lut_restore = golden_apply(&p, vec![remove("c1", "fx2")]).inverse[0].clone();
    assert_eq!(lut_restore.ids(), ["c1", "fx2", "a2"]);
    ids_cover_every_id_in_json(&lut_restore);
    // Move.
    let a = golden_apply(&p, vec![mv("c1", "fx1", 1)]);
    assert_eq!(effect_ids(&a.project, "c1"), ["fx2", "fx1"]);
    let [c] = &a.diff.changes[..] else {
        panic!("{:?}", a.diff)
    };
    assert_eq!(
        (c.kind, c.id.as_str(), c.path.as_deref()),
        (EntityKind::Item, "c1", Some("effects"))
    );
    assert!(a.diff.shifts.is_empty());
}

#[test]
fn keyframe_ops_golden() {
    let p = busy();
    // A new keyframe is placed in time order; its inverse removes it.
    let a = golden_apply(&p, vec![key("c1", "opacity", secs(4), 0.5)]);
    let times: Vec<Rational> = keys(&a.project, "c1", "opacity")
        .iter()
        .map(|k| k.0)
        .collect();
    assert_eq!(times, [secs(2), secs(4), secs(6)]);
    assert!(matches!(&a.inverse[0], Op::KeyframeRemove(_)));
    let [c] = &a.diff.changes[..] else {
        panic!("{:?}", a.diff)
    };
    assert_eq!(
        (c.kind, c.id.as_str(), c.path.as_deref(), c.track.as_deref()),
        (EntityKind::Item, "c1", Some("opacity@4"), Some("t1"))
    );
    // On an effect's parameter the change is the effect's.
    let a = golden_apply(&p, vec![key("c1", "effects.fx1.radius", secs(2), 5.0)]);
    assert_eq!(
        (a.diff.changes[0].kind, a.diff.changes[0].id.as_str()),
        (EntityKind::Effect, "c1/fx1")
    );
    // The first keyframe of a path, and a vec2 with a bézier.
    let op: Op = KeyframeSet {
        item: "c2".into(),
        path: "transform.position".into(),
        at: r("1/2"),
        value: ParamValue::Vec2([1.0, -2.0]),
        interp: Interp::Bezier {
            x1: 0.25,
            y1: 0.1,
            x2: 0.25,
            y2: 1.0,
        },
    }
    .into();
    ids_cover_every_id_in_json(&op);
    let a = golden_apply(&p, vec![op]);
    assert_eq!(keys(&a.project, "c2", "transform.position").len(), 1);
    // Remove: the last keyframe of a path takes the path's list along.
    let a = golden_apply(&p, vec![unkey("c1", "opacity", secs(2))]);
    assert_eq!(
        keys(&a.project, "c1", "opacity"),
        [(secs(6), ParamValue::Number(0.0))]
    );
    let a = golden_apply(
        &p,
        vec![
            unkey("c1", "opacity", secs(2)),
            unkey("c1", "opacity", secs(6)),
        ],
    );
    assert!(
        !item(&a.project, "c1")
            .common
            .keyframes
            .contains_key("opacity")
    );
    assert_eq!(
        try_apply(&p, vec![unkey("c1", "opacity", secs(3))])
            .unwrap_err()
            .code,
        "NOT_FOUND"
    );
}

#[test]
fn keyframe_set_replaces_at_same_time() {
    let p = busy();
    let op: Op = KeyframeSet {
        item: "c1".into(),
        path: "opacity".into(),
        at: secs(2),
        value: ParamValue::Number(0.25),
        interp: Interp::Hold,
    }
    .into();
    let a = golden_apply(&p, vec![op]);
    let k = &item(&a.project, "c1").common.keyframes["opacity"];
    assert_eq!(k.len(), 2);
    assert_eq!(
        k[0],
        Keyframe {
            at: secs(2),
            value: ParamValue::Number(0.25),
            interp: Interp::Hold,
        }
    );
    // The inverse puts the old keyframe back.
    let Op::KeyframeSet(back) = &a.inverse[0] else {
        panic!("{:?}", a.inverse)
    };
    assert_eq!(
        (back.value.clone(), back.interp),
        (ParamValue::Number(1.0), Interp::Linear)
    );
}

#[test]
fn keyframe_on_static_path_refused() {
    let p = busy();
    let blend: Op = KeyframeSet {
        item: "c1".into(),
        path: "blend".into(),
        at: secs(1),
        value: ParamValue::Text("screen".into()),
        interp: Interp::Hold,
    }
    .into();
    let e = try_apply(&p, vec![blend.clone()]).unwrap_err();
    assert_eq!(e.code, "PARAM_NOT_ANIMATABLE");
    assert_eq!(e.message, "blend cannot be animated");
    assert_eq!(e.details["path"], "blend");
    assert_eq!(e.fix.as_deref(), Some("set it with param.set"));
    // Policy: replay applies it. An unknown path or a value of the wrong type is structural.
    let t = tx(vec![blend]);
    apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Replay).unwrap();
    let e = try_apply(&p, vec![key("c1", "nope", secs(1), 1.0)]).unwrap_err();
    assert_eq!(e.code, "PARAM_UNKNOWN");
    let t = tx(vec![
        KeyframeSet {
            item: "c1".into(),
            path: "opacity".into(),
            at: secs(1),
            value: ParamValue::Text("x".into()),
            interp: Interp::Linear,
        }
        .into(),
    ]);
    let e = apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Replay).unwrap_err();
    assert_eq!(e.code, "PARAM_TYPE");
    // Ranges: the value, `at` inside the item, bézier x in [0, 1].
    let e = try_apply(&p, vec![key("c1", "opacity", secs(1), 2.0)]).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    let e = try_apply(&p, vec![key("c1", "opacity", secs(11), 1.0)]).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(
        e.details,
        json!({ "item": "c1", "path": "opacity", "at": "11", "min": "0",
                                  "max": "10", "unit": "s", "opIndex": 0 })
    );
    let bad: Op = KeyframeSet {
        item: "c1".into(),
        path: "opacity".into(),
        at: secs(1),
        value: ParamValue::Number(0.5),
        interp: Interp::Bezier {
            x1: 1.5,
            y1: 0.0,
            x2: 0.5,
            y2: 1.0,
        },
    }
    .into();
    assert_eq!(try_apply(&p, vec![bad]).unwrap_err().code, "INVALID_RANGE");
    // A static path's set is refused once keyframes animate it (param.set).
    assert_eq!(
        try_apply(&p, vec![set("c1", "effects.fx1.radius", 3.0)])
            .unwrap_err()
            .code,
        "PARAM_ANIMATED"
    );
}

#[test]
fn effect_rules() {
    let p = busy();
    let e = try_apply(&p, vec![add("c1", "$fx", "glow", None)]).unwrap_err();
    assert_eq!(e.code, "PARAM_UNKNOWN");
    let mut params = BTreeMap::new();
    params.insert("radius".to_string(), ParamValue::Number(300.0));
    let op: Op = EffectAdd {
        item: "c1".into(),
        id: "$fx".into(),
        kind: "blur".into(),
        at: None,
        params,
    }
    .into();
    let e = try_apply(&p, vec![op]).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(e.details["path"], "effects.$fx.radius");
    assert_eq!(
        try_apply(&p, vec![add("c1", "$fx", "blur", Some(3))])
            .unwrap_err()
            .code,
        "INVALID_RANGE"
    );
    assert_eq!(
        try_apply(&p, vec![mv("c1", "fx1", 2)]).unwrap_err().code,
        "INVALID_RANGE"
    );
    assert_eq!(
        try_apply(&p, vec![remove("c1", "fx9")]).unwrap_err().code,
        "NOT_FOUND"
    );
    // Clients never send a restore.
    let restore: Op = EffectRestore {
        item: "c2".into(),
        effect: Effect {
            id: "fx7".into(),
            kind: "blur".into(),
            enabled: true,
            params: BTreeMap::new(),
        },
        index: 0,
        keyframes: BTreeMap::new(),
    }
    .into();
    assert_eq!(try_apply(&p, vec![restore]).unwrap_err().code, "INVALID_OP");
    // Locked tracks refuse every effect and keyframe op.
    let mut locked = p.clone();
    locked.timelines[0].tracks[0].lock = true;
    for op in [
        add("c1", "$fx", "blur", None),
        remove("c1", "fx1"),
        mv("c1", "fx1", 1),
        key("c1", "opacity", secs(1), 1.0),
        unkey("c1", "opacity", secs(2)),
    ] {
        assert_eq!(
            try_apply(&locked, vec![op]).unwrap_err().code,
            "TRACK_LOCKED"
        );
    }
}

#[test]
fn effect_ids_come_from_the_global_counter_and_split_keeps_them() {
    let p = with_clips(&[10, 10]);
    let a = apply(
        &p,
        vec![
            add("c1", "$x", "blur", None),
            add("c2", "$y", "blur", None),
            ItemSplit {
                item: "c1".into(),
                offset: secs(4),
                id: "$r".into(),
            }
            .into(),
            add("$r", "$z", "lut", None),
        ],
    );
    // One counter for the project, not one per item.
    assert_eq!(
        (a.bindings["$x"].as_str(), a.bindings["$y"].as_str()),
        ("fx1", "fx2")
    );
    let right = &a.bindings["$r"];
    // The right part keeps the left's effect ids: an effect is (item, effect).
    assert_eq!(effect_ids(&a.project, "c1"), ["fx1"]);
    assert_eq!(effect_ids(&a.project, right), ["fx1", "fx3"]);
    let b = apply(&a.project, vec![set(right, "effects.fx1.radius", 9.0)]);
    assert_eq!(
        params::get(item(&b.project, "c1"), "effects.fx1.radius").unwrap(),
        None
    );
    assert_eq!(b.diff.changes[0].id, format!("{right}/fx1"));
}

/// Decision taken here: a split keeps every keyframe on both parts (the right part's moved by
/// `−offset`), so the left part keeps the keyframes past the cut, out of its range. Each part
/// animates over its own range exactly as before (interpolation across the cut included), and
/// the split's inverse (a strict trim and a lift) restores the item exactly.
#[test]
fn split_keeps_every_keyframe_on_both_parts() {
    let p = busy();
    let a = golden_apply(
        &p,
        vec![
            ItemSplit {
                item: "c1".into(),
                offset: secs(4),
                id: "$r".into(),
            }
            .into(),
        ],
    );
    let right = &a.bindings["$r"];
    assert_eq!(
        keys(&a.project, "c1", "opacity"),
        [
            (secs(2), ParamValue::Number(1.0)),
            (secs(6), ParamValue::Number(0.0))
        ]
    );
    assert_eq!(
        keys(&a.project, right, "opacity"),
        [
            (secs(-2), ParamValue::Number(1.0)),
            (secs(2), ParamValue::Number(0.0))
        ]
    );
    // A keyframe past the left part's end can still be removed (no range check on removal).
    golden_apply(&a.project, vec![unkey("c1", "opacity", secs(6))]);
}

// ---- rebase ----

fn commit(p: &Project, ops: Vec<Op>, rev: u64, tx_id: &str) -> (Project, CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = tx_id.into();
    t.base_rev = rev - 1;
    let a = apply_tx(p, &t, &mut MemIds::after(p)).unwrap_or_else(|e| panic!("{tx_id}: {e}"));
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

fn rebase_ops(base: &Project, theirs: Vec<Op>, ours: Vec<Op>) -> Result<Project, CoreError> {
    let (head, c) = commit(base, theirs, 1, "tx-a");
    let mut t = tx(ours);
    t.tx_id = "tx-b".into();
    rebase_tx(base, &head, &t, &[c], &mut MemIds::after(&head)).map(|r| r.applied.project)
}

fn reason(e: &CoreError) -> String {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    e.details["reason"].as_str().unwrap().to_string()
}

/// B3: the stale `keyframe.set` at 6 after a concurrent start trim by 1 replaces the keyframe
/// now at 5: the trim moved every keyframe of the item by the same `by`.
#[test]
fn stale_keyframe_after_start_trim_keeps_identity() {
    let p = busy();
    let got = rebase_ops(
        &p,
        vec![trim("c1", Edge::Start, secs(1))],
        vec![key("c1", "opacity", secs(6), 0.75)],
    )
    .unwrap();
    assert_eq!(
        keys(&got, "c1", "opacity"),
        [
            (secs(1), ParamValue::Number(1.0)),
            (secs(5), ParamValue::Number(0.75))
        ]
    );
    // Effect keyframes too.
    let got = rebase_ops(
        &p,
        vec![trim("c1", Edge::Start, secs(-2))],
        vec![key("c1", "effects.fx1.radius", secs(4), 7.0)],
    )
    .unwrap();
    assert_eq!(
        keys(&got, "c1", "effects.fx1.radius"),
        [
            (secs(3), ParamValue::Number(0.0)),
            (secs(6), ParamValue::Number(7.0))
        ]
    );
}

#[test]
fn stale_keyframe_remove_after_start_trim() {
    let p = busy();
    let got = rebase_ops(
        &p,
        vec![trim("c1", Edge::Start, secs(1))],
        vec![unkey("c1", "opacity", secs(2))],
    )
    .unwrap();
    assert_eq!(
        keys(&got, "c1", "opacity"),
        [(secs(5), ParamValue::Number(0.0))]
    );
}

/// An undo of an effect removal after a start trim puts its keyframes back where the trim
/// moved them (the restore's keyframe times are content offsets too).
#[test]
fn stale_effect_restore_after_start_trim() {
    let p = busy();
    let mut log = MemLog::new(p);
    log.commit("main", "tx-r", "alice", vec![remove("c1", "fx1")]);
    log.commit(
        "main",
        "tx-t",
        "bob",
        vec![trim("c1", Edge::Start, secs(1))],
    );
    log.undo("main", "alice", "tx-u").unwrap();
    let got = log.state("main");
    assert_eq!(effect_ids(&got, "c1"), ["fx1", "fx2"]);
    assert_eq!(
        keys(&got, "c1", "effects.fx1.radius"),
        [
            (secs(0), ParamValue::Number(0.0)),
            (secs(3), ParamValue::Number(20.0))
        ]
    );
}

#[test]
fn concurrent_keyframes_at_different_times_merge() {
    let p = busy();
    let got = rebase_ops(
        &p,
        vec![key("c1", "opacity", secs(3), 0.5)],
        vec![key("c1", "opacity", secs(5), 0.25)],
    )
    .unwrap();
    assert_eq!(keys(&got, "c1", "opacity").len(), 4);
}

#[test]
fn same_keyframe_conflicts_same_param() {
    let p = busy();
    let e = rebase_ops(
        &p,
        vec![key("c1", "opacity", secs(2), 0.5)],
        vec![key("c1", "opacity", secs(2), 0.25)],
    )
    .unwrap_err();
    assert_eq!(reason(&e), "SAME_PARAM");
    let e = rebase_ops(
        &p,
        vec![unkey("c1", "opacity", secs(2))],
        vec![key("c1", "opacity", secs(2), 0.25)],
    )
    .unwrap_err();
    assert_eq!(reason(&e), "SAME_PARAM");
}

#[test]
fn effect_move_vs_move_same_param() {
    let p = busy();
    let e = rebase_ops(&p, vec![mv("c1", "fx1", 1)], vec![mv("c1", "fx2", 0)]).unwrap_err();
    assert_eq!(reason(&e), "SAME_PARAM");
    // Appends of two effects merge; one at an index meets a move.
    let got = rebase_ops(
        &p,
        vec![add("c1", "$a", "blur", None)],
        vec![add("c1", "$b", "colorAdjust", None)],
    )
    .unwrap();
    assert_eq!(effect_ids(&got, "c1"), ["fx1", "fx2", "fx3", "fx4"]);
    let e = rebase_ops(
        &p,
        vec![mv("c1", "fx1", 1)],
        vec![add("c1", "$b", "blur", Some(0))],
    )
    .unwrap_err();
    assert_eq!(reason(&e), "SAME_PARAM");
}

#[test]
fn edits_of_a_removed_effect_conflict_removed() {
    let p = busy();
    for ours in [
        set("c1", "effects.fx2.intensity", 0.5),
        key("c1", "effects.fx1.radius", secs(2), 1.0),
        mv("c1", "fx1", 1),
        remove("c1", "fx1"),
    ] {
        let theirs = match &ours {
            Op::ParamSet(_) => remove("c1", "fx2"),
            _ => remove("c1", "fx1"),
        };
        let e = rebase_ops(&p, vec![theirs], vec![ours.clone()]).unwrap_err();
        assert_eq!(reason(&e), "REMOVED", "{ours:?}");
    }
    // An effect edit on a deleted item is REMOVED too.
    let e = rebase_ops(&p, vec![delete("c1")], vec![add("c1", "$x", "blur", None)]).unwrap_err();
    assert_eq!(reason(&e), "REMOVED");
    // Edits of different effects merge.
    let off: Op = ParamSet {
        item: "c1".into(),
        path: "effects.fx1.enabled".into(),
        value: Some(ParamValue::Bool(false)),
    }
    .into();
    let got = rebase_ops(&p, vec![remove("c1", "fx2")], vec![off]).unwrap();
    assert_eq!(effect_ids(&got, "c1"), ["fx1"]);
    assert!(!item(&got, "c1").common.effects[0].enabled);
}

// ---- review of Tasks 11/14 ----

/// B1: a later op of the transaction names a new effect by its placeholder inside a path
/// (`effects.$b.<param>`); it resolves at apply and in the resolved ops, and the inverse names
/// the bound id.
#[test]
fn effect_placeholder_resolves_inside_paths() {
    let p = busy();
    let off: Op = ParamSet {
        item: "c2".into(),
        path: "effects.$b.enabled".into(),
        value: Some(ParamValue::Bool(false)),
    }
    .into();
    let ops = vec![
        add("c2", "$b", "blur", None),
        key("c2", "effects.$b.radius", secs(1), 4.0),
        off.clone(),
    ];
    assert_eq!(ops[1].ids(), ["c2", "$b"]);
    let a = golden_apply(&p, ops.clone());
    let fx = a.bindings["$b"].clone();
    assert_eq!(fx, "fx3");
    let path = format!("effects.{fx}.radius");
    assert_eq!(
        keys(&a.project, "c2", &path),
        [(secs(1), ParamValue::Number(4.0))]
    );
    assert!(!item(&a.project, "c2").common.effects[0].enabled);
    let resolved = fin_core::resolve_ops(&ops, &a.bindings).unwrap();
    assert_eq!(resolved[1], key("c2", &path, secs(1), 4.0));
    assert!(
        a.inverse.iter().all(|op| !format!("{op:?}").contains('$')),
        "{:?}",
        a.inverse
    );
    // An unbound one is UNBOUND_PLACEHOLDER.
    let e = apply_tx(
        &p,
        &tx(vec![key("c2", "effects.$z.radius", secs(1), 4.0)]),
        &mut MemIds::after(&p),
    )
    .unwrap_err();
    assert_eq!(e.code, "UNBOUND_PLACEHOLDER");
}

/// Task 26 (found by `generators_reach_every_op_kind`): `effect.move` and `effect.remove` name
/// an effect added earlier in the transaction by its placeholder; it resolves at apply, and the
/// inverses name the bound id.
#[test]
fn effect_placeholder_resolves_in_move_and_remove() {
    let p = busy();
    let ops = vec![
        add("c2", "$b", "blur", None),
        add("c2", "$c", "blur", None),
        mv("c2", "$c", 0),
        remove("c2", "$b"),
    ];
    let a = golden_apply(&p, ops);
    assert_eq!(effect_ids(&a.project, "c2"), [a.bindings["$c"].clone()]);
    assert!(
        a.inverse.iter().all(|op| !format!("{op:?}").contains('$')),
        "{:?}",
        a.inverse
    );
}

/// B1: `for_each_ref` reaches the effect segment of a path and of a keyframe map's keys, and
/// renames keys together (a swap keeps both lists).
#[test]
fn for_each_ref_renames_effect_paths() {
    let mut op = remove_inverse(&busy(), "fx1");
    let swap = |r: &mut String| match r.as_str() {
        "fx1" => *r = "fx2".into(),
        "fx2" => *r = "fx1".into(),
        _ => {}
    };
    let Op::EffectRestore(r) = &mut op else {
        panic!()
    };
    r.keyframes.insert(
        "effects.fx2.radius".into(),
        r.keyframes["effects.fx1.radius"][..1].to_vec(),
    );
    op.for_each_ref(swap);
    let Op::EffectRestore(r) = &op else { panic!() };
    assert_eq!(r.effect.id, "fx2");
    assert_eq!(r.keyframes["effects.fx2.radius"].len(), 2);
    assert_eq!(r.keyframes["effects.fx1.radius"].len(), 1);
    let mut set = key("c1", "effects.fx1.radius", secs(1), 1.0);
    set.for_each_ref(swap);
    assert_eq!(set, key("c1", "effects.fx2.radius", secs(1), 1.0));
    assert_eq!(set.ids(), ["c1", "fx2"]);
}

fn remove_inverse(p: &Project, fx: &str) -> Op {
    apply(p, vec![remove("c1", fx)]).inverse.remove(0)
}

/// `c1` with `fx1`..`fx3`.
fn three_effects() -> Project {
    apply(&busy(), vec![add("c1", "$c", "colorAdjust", None)]).project
}

/// Replays every record of `main` from its genesis (what a rebuild does).
fn replays(log: &MemLog, genesis: &Project, n: u64) {
    use fin_core::history::LogView;
    let mut p = genesis.clone();
    for rec in log.records("main", 0, n).unwrap() {
        p = fin_core::apply_committed(&p, &rec.record.intent, &rec.record.bindings)
            .unwrap_or_else(|e| panic!("seq {}: {e:?}", rec.seq))
            .project;
    }
    assert_eq!(p, log.state("main"));
}

/// S1: Alice removes fx3, Bob removes fx1, Alice undoes: the restore's index is past the end
/// of the shorter list, so fx3 goes back at the end (and the undo replays).
#[test]
fn undo_of_effect_remove_after_another_removal() {
    let p = three_effects();
    let mut log = MemLog::new(p.clone());
    log.commit("main", "tx-a", "alice", vec![remove("c1", "fx3")]);
    log.commit("main", "tx-b", "bob", vec![remove("c1", "fx1")]);
    log.undo("main", "alice", "tx-u").unwrap();
    assert_eq!(effect_ids(&log.state("main"), "c1"), ["fx2", "fx3"]);
    replays(&log, &p, 3);
    // The inverse of a move too: fx3 to the front, fx1 removed, undo moves fx3 to the end.
    let mut log = MemLog::new(p.clone());
    log.commit("main", "tx-a", "alice", vec![mv("c1", "fx3", 0)]);
    log.commit("main", "tx-b", "bob", vec![remove("c1", "fx1")]);
    log.undo("main", "alice", "tx-u").unwrap();
    assert_eq!(effect_ids(&log.state("main"), "c1"), ["fx2", "fx3"]);
    replays(&log, &p, 3);
}

/// S1: a re-run that fails on an item's effect list blames the one step that changed it,
/// even when that step only removed another effect.
#[test]
fn effect_list_rerun_blames_the_removal() {
    let p = three_effects();
    let e = rebase_ops(&p, vec![remove("c1", "fx1")], vec![mv("c1", "fx3", 2)]).unwrap_err();
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    assert_eq!(e.details["againstTx"], "tx-a", "{e:?}");
    // A client's move past the end stays INVALID_RANGE.
    let e = apply_tx(&p, &tx(vec![mv("c1", "fx3", 3)]), &mut MemIds::after(&p)).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
}

/// `effect.restore` keyframe lists are non-empty and strictly in time order (structural).
#[test]
fn effect_restore_checks_keyframe_lists() {
    let p = busy();
    let a = apply(&p, vec![remove("c1", "fx1")]);
    let Op::EffectRestore(good) = &a.inverse[0] else {
        panic!()
    };
    let path = "effects.fx1.radius";
    let mut bad = [(**good).clone(), (**good).clone(), (**good).clone()];
    bad[0].keyframes.get_mut(path).unwrap().reverse();
    let dup = bad[1].keyframes[path][0].clone();
    bad[1].keyframes.get_mut(path).unwrap().push(dup);
    bad[2].keyframes.get_mut(path).unwrap().clear();
    for r in bad {
        for mode in [ApplyMode::Internal, ApplyMode::Replay] {
            let e = apply_tx_mode(
                &a.project,
                &tx(vec![r.clone().into()]),
                &mut MemIds::after(&p),
                mode,
            )
            .unwrap_err();
            assert_eq!(e.code, "INVALID_OP", "{e:?}");
        }
    }
}

/// Built-in effects are visual: refused on an audio track's item (policy), replayed as stored.
#[test]
fn visual_effects_refused_on_audio_items() {
    let p = apply(
        &busy(),
        vec![insert("$x", "t2", secs(0), "a1", secs(0), secs(2))],
    )
    .project;
    let op = add("c3", "$b", "blur", None);
    let e = apply_tx(&p, &tx(vec![op.clone()]), &mut MemIds::after(&p)).unwrap_err();
    assert_eq!(e.code, "TRACK_KIND_MISMATCH");
    assert_eq!(e.details["trackKind"], "audio");
    let mut literal = op;
    literal.for_each_ref(|r| {
        if r == "$b" {
            *r = "fx9".into();
        }
    });
    apply_tx_mode(
        &p,
        &tx(vec![literal]),
        &mut MemIds::after(&p),
        ApplyMode::Replay,
    )
    .unwrap();
}

/// A path's first keyframe (it stops being static) meets a concurrent `param.set` of it, and
/// removing its last keyframe does too.
#[test]
fn first_keyframe_meets_a_static_set() {
    let p = busy();
    for (theirs, ours) in [
        (
            set("c2", "opacity", 0.5),
            key("c2", "opacity", secs(1), 0.25),
        ),
        (
            key("c2", "opacity", secs(1), 0.25),
            set("c2", "opacity", 0.5),
        ),
    ] {
        let e = rebase_ops(&p, vec![theirs], vec![ours]).unwrap_err();
        assert_eq!(reason(&e), "SAME_PARAM");
    }
    let one = apply(&p, vec![key("c2", "opacity", secs(1), 0.25)]).project;
    let e = rebase_ops(
        &one,
        vec![unkey("c2", "opacity", secs(1))],
        vec![set("c2", "opacity", 0.5)],
    )
    .unwrap_err();
    assert_eq!(reason(&e), "SAME_PARAM");
    // A keyframe on a path already animated stays at its own time.
    let got = rebase_ops(
        &p,
        vec![set("c2", "opacity", 0.5)],
        vec![key("c1", "opacity", secs(3), 0.5)],
    );
    assert!(got.is_ok());
}

/// S6: a bézier's `y1`/`y2` are bounded (policy), so an eased value stays renderable; replay
/// keeps what was stored.
#[test]
fn bezier_y_is_bounded() {
    let p = busy();
    let eased = |y1: f64| -> Op {
        KeyframeSet {
            item: "c2".into(),
            path: "opacity".into(),
            at: secs(1),
            value: ParamValue::Number(0.5),
            interp: Interp::Bezier {
                x1: 0.5,
                y1,
                x2: 0.5,
                y2: 1.0,
            },
        }
        .into()
    };
    apply(&p, vec![eased(100.0)]);
    let e = apply_tx(&p, &tx(vec![eased(1e300)]), &mut MemIds::after(&p)).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["max"].as_f64()),
        ("INVALID_RANGE", Some(100.0))
    );
    apply_tx_mode(
        &p,
        &tx(vec![eased(1e300)]),
        &mut MemIds::after(&p),
        ApplyMode::Replay,
    )
    .unwrap();
}
