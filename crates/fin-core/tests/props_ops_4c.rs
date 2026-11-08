//! Op properties over every 4C op (plan 4C Task 26, props 1, 2 and 6), on a base project that
//! holds every kind of 4C entity ([`gen4c::base4c`]).
//!
//! - Prop 1: a chain of transactions of one to four ops each, of every client and internal op
//!   (`strict` trims, `item.remove|restore`, the font ops included). The tx is its ops in a
//!   row; its stored inverse takes the project back exactly; the inverse's own inverse (a redo)
//!   gives the tx's result again; the inverse records the tx's shifts inverted in exact reverse
//!   order (Decision 23, B5); a replay of the record reproduces state, diff and inverse; and
//!   every op, its resolved form and every inverse op list each id they carry (B6).
//! - Prop 2: rebasing a client tx over an empty interval is applying it.
//! - Prop 6: any 4C op (any ids, any times, any mode), no-shift ops past their policy limits
//!   in `Replay` included (B1): `apply_tx_mode` never panics, and a success keeps the model's
//!   structural invariants (policy ones too outside `Replay`).

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::gen4c::{self, G, OP_NAMES, World, base4c, frames, g_any, shape};
use common::ids_json::ids_cover_every_id_in_json;
use common::*;
use fin_core::rebase::StepMap;
use fin_core::{
    ApplyMode, CaptionMerge, CaptionSetText, CaptionSplit, DeleteMode, Edge, EffectMove,
    InsertMode, Item, ItemBody, ItemDelete, ItemInsert, ItemLink, ItemMove, ItemRemove,
    ItemRestore, ItemSlide, ItemSlip, ItemSplit, ItemTrim, KeyframeSet, MemIds, Op, ParamSet,
    ParamValue, Project, Rational, Shift, TrackKind, TransitionKind, TransitionSet, TrimMode,
    apply_internal, apply_tx, apply_tx_mode, frame_duration, rebase_tx, resolve_ops,
};
use proptest::prelude::*;
use proptest::sample::Index;

fn mode_for(ops: &[Op]) -> ApplyMode {
    if ops.iter().any(Op::is_internal) {
        ApplyMode::Internal
    } else {
        ApplyMode::Client
    }
}

fn strip(s: &[Shift]) -> Vec<(String, Rational, Rational)> {
    s.iter().map(|s| (s.track.clone(), s.from, s.by)).collect()
}

/// One tx of prop 1 at `p`: every check of the module docs; returns the state after it.
fn exact(p: &Project, w: &World) -> Result<Project, TestCaseError> {
    let t = tx(w.ops.clone());
    let a = apply_tx_mode(p, &t, &mut MemIds::after(p), mode_for(&w.ops))
        .unwrap_or_else(|e| panic!("{:?}: {e:?}", w.ops));
    prop_assert_eq!(&a.project, &w.state, "the tx is its ops in a row");
    let resolved = resolve_ops(&w.ops, &a.bindings).unwrap();
    for op in w.ops.iter().chain(&resolved).chain(&a.inverse) {
        ids_cover_every_id_in_json(op);
    }
    let back = apply_internal(
        &a.project,
        &tx(a.inverse.clone()),
        &mut MemIds::after(&a.project),
    )
    .unwrap_or_else(|e| panic!("inverse {:?}: {e:?}", a.inverse));
    prop_assert_eq!(&back.project, p, "inverse of {:?}", w.ops);
    let want = StepMap::from_diff(&a.diff).invert().unwrap();
    prop_assert_eq!(
        strip(&back.diff.shifts),
        strip(want.shifts()),
        "inverse shifts mirror {:?}",
        w.ops
    );
    let again = apply_internal(p, &tx(back.inverse.clone()), &mut MemIds::after(p))
        .unwrap_or_else(|e| panic!("redo {:?}: {e:?}", back.inverse));
    prop_assert_eq!(&again.project, &a.project, "redo of {:?}", w.ops);
    let rp = fin_core::replay::apply_committed(p, &t, &a.bindings).unwrap();
    prop_assert_eq!(&rp.project, &a.project, "replay of {:?}", w.ops);
    prop_assert_eq!(&rp.diff, &a.diff);
    prop_assert_eq!(&rp.inverse, &a.inverse);
    Ok(a.project)
}

proptest! {
    #![proptest_config(prop_config(256))]
    /// Prop 1 (module docs).
    #[test]
    fn apply_then_inverse_is_exact_for_every_4c_op(
        shape in shape(),
        txs in prop::collection::vec(prop::collection::vec(g_any(), 1..5), 1..5),
    ) {
        let mut p = base4c(&shape);
        let mut graveyard = Vec::new();
        for gs in &txs {
            let mut w = World::new(&p, MemIds::after(&p), true);
            w.graveyard = std::mem::take(&mut graveyard);
            w.run(gs);
            graveyard = std::mem::take(&mut w.graveyard);
            if w.ops.is_empty() {
                continue;
            }
            p = exact(&p, &w)?;
        }
    }
}

proptest! {
    #![proptest_config(prop_config(256))]
    /// Prop 2: rebasing over nothing is applying at the head.
    #[test]
    fn rebase_over_empty_interval_is_identity_4c(
        shape in shape(),
        gs in prop::collection::vec(g_any(), 1..6),
    ) {
        let p = base4c(&shape);
        let mut w = World::new(&p, MemIds::after(&p), false);
        w.run(&gs);
        if w.ops.is_empty() {
            return Ok(());
        }
        let t = tx(w.ops);
        let ids = MemIds::after(&p);
        let fresh = apply_tx(&p, &t, &mut ids.clone()).unwrap();
        let rb = rebase_tx(&p, &p, &t, &[], &mut ids.clone()).unwrap();
        prop_assert_eq!(rb.tx, t);
        prop_assert_eq!(rb.applied, fresh);
    }
}

// ---------------------------------------------------------------------------------------------
// Prop 6.

/// An arbitrary op: a template, entity picks (from the project's ids and junk), times and flags.
#[derive(Debug, Clone)]
struct AnyOp {
    template: u8,
    ids: [Index; 3],
    times: [Rational; 2],
    flags: u8,
}

fn any_time() -> impl Strategy<Value = Rational> {
    prop_oneof![
        6 => (-120i64..400).prop_map(frames),
        2 => (-5i64..5, prop::sample::select(vec![1i64, 2, 7, 1001, 48_000]))
            .prop_map(|(n, d)| Rational::new(n, d).unwrap()),
        1 => prop::sample::select(vec![i64::MAX, i64::MIN + 1, i64::MAX / 3, 1 << 40])
            .prop_map(Rational::from_int),
    ]
}

fn any_op() -> impl Strategy<Value = AnyOp> {
    (
        0u8..18,
        [any::<Index>(), any::<Index>(), any::<Index>()],
        [any_time(), any_time()],
        any::<u8>(),
    )
        .prop_map(|(template, ids, times, flags)| AnyOp {
            template,
            ids,
            times,
            flags,
        })
}

const JUNK: &[&str] = &["c99", "$x", "$", "", "x", "C1", "t9", "tl9", "fx9", "lk9"];

impl AnyOp {
    fn build(&self, p: &Project) -> Op {
        let mut pool: Vec<String> = p.entity_ids().into_iter().map(String::from).collect();
        pool.extend(JUNK.iter().map(|s| s.to_string()));
        let id = |i: usize| self.ids[i].get(&pool).clone();
        let (t0, t1) = (self.times[0], self.times[1]);
        let f = self.flags;
        let bit = |b: u8| f & (1 << b) != 0;
        let delete = || {
            if bit(1) {
                DeleteMode::Lift
            } else {
                DeleteMode::Ripple
            }
        };
        let body = |asset: String| ItemBody::Clip {
            asset,
            source_in: t1,
            speed: Rational::from_int(1),
        };
        match self.template {
            0 => ItemInsert {
                id: "$n".into(),
                track: id(0),
                at: t0,
                duration: t1,
                mode: if bit(0) {
                    InsertMode::Overwrite
                } else {
                    InsertMode::Ripple
                },
                body: body(id(1)),
            }
            .into(),
            1 => ItemTrim {
                item: id(0),
                edge: if bit(0) { Edge::Start } else { Edge::End },
                delta: t0,
                mode: [
                    TrimMode::Ripple,
                    TrimMode::Roll,
                    TrimMode::Overwrite,
                    TrimMode::Strict,
                ][(f >> 2) as usize % 4],
                alone: bit(4),
            }
            .into(),
            2 => ItemDelete {
                item: id(0),
                mode: delete(),
            }
            .into(),
            3 => ItemRemove {
                id: id(0),
                mode: delete(),
            }
            .into(),
            4 => {
                // An existing item (or a fresh one) put back anywhere, overlapping or not.
                let mut item = p
                    .timelines
                    .iter()
                    .flat_map(|t| &t.tracks)
                    .flat_map(|t| &t.items)
                    .nth(self.ids[1].index(64))
                    .cloned()
                    .unwrap_or_else(|| Item::clip(id(1), id(2), t0, t1, t1));
                item.start = t0;
                if bit(0) {
                    item.id = "$r".into();
                }
                ItemRestore {
                    item,
                    track: id(0),
                    mode: delete(),
                }
                .into()
            }
            5 => ItemSlip {
                item: id(0),
                delta: t0,
            }
            .into(),
            6 => ItemSplit {
                item: id(0),
                offset: t0,
                id: "$s".into(),
            }
            .into(),
            7 => ItemMove {
                item: id(0),
                track: id(1),
                at: t0,
                mode: if bit(0) {
                    InsertMode::Overwrite
                } else {
                    InsertMode::Ripple
                },
            }
            .into(),
            8 => ItemSlide {
                item: id(0),
                delta: t0,
                sides: None,
            }
            .into(),
            9 => ItemLink {
                items: vec![id(0), id(1)],
                id: if bit(0) { "$l".into() } else { id(2) },
            }
            .into(),
            10 => TransitionSet {
                id: if bit(0) { "$tr".into() } else { id(2) },
                track: id(0),
                between: [id(1), id(2)],
                kind: if bit(1) {
                    TransitionKind::Dissolve
                } else {
                    TransitionKind::Crossfade
                },
                duration: t0,
                curve: fin_core::Curve::Linear,
            }
            .into(),
            11 => ParamSet {
                item: id(0),
                path: [
                    "opacity",
                    "transform.rotation",
                    "audio.gain",
                    "speed",
                    "crop.left",
                ][f as usize % 5]
                    .into(),
                value: bit(5).then(|| ParamValue::Number(t0.num() as f64)),
            }
            .into(),
            12 => KeyframeSet {
                item: id(0),
                path: "opacity".into(),
                at: t0,
                value: ParamValue::Number(0.5),
                interp: fin_core::Interp::Linear,
            }
            .into(),
            13 => EffectMove {
                item: id(0),
                effect: id(1),
                to: u32::from(f % 4),
            }
            .into(),
            14 => CaptionSplit {
                item: id(0),
                word: u32::from(f % 4),
                id: "$c".into(),
            }
            .into(),
            15 => CaptionMerge {
                item: id(0),
                with: id(1),
            }
            .into(),
            16 => CaptionSetText {
                item: id(0),
                text: ["", "a b", "x"][f as usize % 3].into(),
            }
            .into(),
            _ => gen4c::pin(
                gen4c::INTER,
                gen4c::inter(&gen4c::sha('a')),
                bit(0).then_some(u32::from(f % 3)),
            ),
        }
    }
}

fn any_mode() -> impl Strategy<Value = ApplyMode> {
    prop_oneof![
        2 => Just(ApplyMode::Client),
        1 => Just(ApplyMode::Internal),
        2 => Just(ApplyMode::Replay),
    ]
}

/// The model invariants (prop 6): structural ones always, policy ones unless `mode` is
/// `Replay` (which skips policy by design).
fn assert_invariants(p: &Project, mode: ApplyMode) -> Result<(), TestCaseError> {
    let mut seen = BTreeSet::new();
    for tl in &p.timelines {
        let frame = frame_duration(tl.rate).unwrap();
        for t in &tl.tracks {
            for c in &t.items {
                prop_assert!(seen.insert(c.id.clone()), "item id {} twice", c.id);
                prop_assert!(c.start >= Rational::zero(), "negative start {:?}", c);
                prop_assert!(c.duration.is_positive(), "empty item {:?}", c);
                let source_in = c.source_in().unwrap_or_default();
                prop_assert!(source_in >= Rational::zero(), "negative sourceIn {:?}", c);
                if mode == ApplyMode::Replay {
                    continue;
                }
                prop_assert!(c.duration >= frame, "shorter than a frame {:?}", c);
                if let ItemBody::Clip { asset, speed, .. } = &c.body {
                    let a = p.assets.get(asset);
                    prop_assert!(a.is_some(), "{} names missing asset {}", c.id, asset);
                    let a = a.unwrap();
                    let fits = match t.kind {
                        TrackKind::Video => a.video.is_some(),
                        TrackKind::Audio => a.audio.is_some(),
                        TrackKind::Caption => false,
                    };
                    prop_assert!(fits, "{} of {} on {:?}", c.id, asset, t.kind);
                    let extent = source_in
                        .checked_add(c.duration.checked_mul(*speed).unwrap())
                        .unwrap();
                    prop_assert!(extent <= a.duration.unwrap(), "past its asset {:?}", c);
                }
            }
            for w in t.items.windows(2) {
                prop_assert!(
                    (w[0].start + w[0].duration).unwrap() <= w[1].start,
                    "overlap or unsorted on {}: {:?} then {:?}",
                    t.id,
                    w[0],
                    w[1]
                );
            }
            for x in &t.transitions {
                let a = t.items.iter().position(|c| c.id == x.between[0]);
                let b = t.items.iter().position(|c| c.id == x.between[1]);
                let at_cut = matches!((a, b), (Some(a), Some(b)) if b == a + 1
                    && (t.items[a].start + t.items[a].duration).unwrap() == t.items[b].start);
                prop_assert!(at_cut, "transition {} on {} not at a cut", x.id, t.id);
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(prop_config(512))]
    /// Prop 6 (module docs).
    #[test]
    fn apply_tx_never_panics_and_success_keeps_invariants_4c(
        shape in shape(),
        ops in prop::collection::vec(any_op(), 1..4),
        mode in any_mode(),
    ) {
        let p = base4c(&shape);
        let before = p.clone();
        let ops: Vec<Op> = ops.iter().map(|o| o.build(&p)).collect();
        let mut ids = MemIds::after(&p);
        match apply_tx_mode(&p, &tx(ops.clone()), &mut ids, mode) {
            Ok(a) => {
                assert_invariants(&a.project, mode)?;
                let ids: BTreeSet<&String> = a.bindings.values().collect();
                prop_assert_eq!(ids.len(), a.bindings.len());
            }
            Err(e) => {
                prop_assert!(!e.code.is_empty());
                prop_assert!(
                    e.details.get("opIndex").is_some() || e.code == "SCHEMA_VERSION",
                    "{:?}", e
                );
            }
        }
        prop_assert_eq!(p, before, "the input is untouched");
    }
}

/// The generators reach every op kind (prop 1's coverage, with a fixed seed): each client and
/// internal op is generated, or is the inverse of a generated tx, at least once.
#[test]
fn generators_reach_every_op_kind() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{RngAlgorithm, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        prop_config(1),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[7; 32]),
    );
    let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    for _ in 0..300 {
        let s = shape().new_tree(&mut runner).unwrap().current();
        let gs: Vec<G> = prop::collection::vec(g_any(), 1..16)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let p = base4c(&s);
        let mut w = World::new(&p, MemIds::after(&p), true);
        w.run(&gs);
        if w.ops.is_empty() {
            continue;
        }
        let a = apply_tx_mode(
            &p,
            &tx(w.ops.clone()),
            &mut MemIds::after(&p),
            mode_for(&w.ops),
        )
        .unwrap_or_else(|e| panic!("{e:?}\n{:#?}", w.ops));
        for op in w.ops.iter().chain(&a.inverse) {
            *kinds.entry(op.name()).or_default() += 1;
        }
    }
    let missing: Vec<&&str> = OP_NAMES
        .iter()
        .filter(|n| !kinds.contains_key(**n))
        .collect();
    assert!(
        missing.is_empty(),
        "never generated: {missing:?}\n{kinds:?}"
    );
}

/// Prop 6's shrunk case (found on 5A's Task 12 run, 2026-11-08; the seed was not kept): template
/// 0 with flags 7, an overwrite `item.insert` at 1/1001 s for 2/5 s on `t2`, whose first item
/// `c4` starts at 0, left `c4` 1/1001 s long (`shorter than a frame`). The overwrite carve now
/// keeps one frame of what it trims (`INVALID_RANGE`, policy); replay still applies it.
#[test]
fn overwrite_at_a_sub_frame_time_keeps_a_frame_4c() {
    let shape = gen4c::Shape {
        lens: vec![vec![12; 3], vec![12; 3], vec![]],
        linked: false,
        transitions: true,
        captions: 1,
        effects: false,
        pinned: false,
    };
    let p = base4c(&shape);
    let mut pool: Vec<String> = p.entity_ids().into_iter().map(String::from).collect();
    pool.extend(JUNK.iter().map(|s| s.to_string()));
    // `Index::index`: a fixed-point multiply of the raw value by the pool's length.
    let pick = |raw: u64| pool[((pool.len() as u128 * u128::from(raw)) >> 64) as usize].clone();
    let (track, asset) = (
        pick(4_471_937_957_262_921_604),
        pick(10_061_860_403_841_573_609),
    );
    assert_eq!(track, "t2");
    let (at, duration) = (
        Rational::new(1, 1001).unwrap(),
        Rational::new(2, 5).unwrap(),
    );
    let op: Op = ItemInsert {
        id: "$n".into(),
        track,
        at,
        duration,
        mode: InsertMode::Overwrite,
        body: ItemBody::Clip {
            asset,
            source_in: duration,
            speed: Rational::from_int(1),
        },
    }
    .into();
    let t = tx(vec![op]);
    let e = apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Client).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["item"]),
        ("INVALID_RANGE", &serde_json::json!("c4")),
        "{e:?}"
    );
    let a = apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Replay).unwrap();
    assert_invariants(&a.project, ApplyMode::Replay).unwrap();
}
