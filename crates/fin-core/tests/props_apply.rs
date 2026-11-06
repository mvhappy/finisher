//! Apply properties (spec §4.6, props 1 and 6; plan 4B Task 20).
//!
//! - Prop 1: a transaction's stored inverse takes the project back exactly, for every op kind:
//!   the client ops, the internal ones (`asset.remove`, `item.remove`, `item.restore`, in
//!   both modes), and an `item.restore` naming its item with a `$` placeholder (a fresh id). The inverse of the
//!   inverse (a redo) gives the transaction's result again.
//! - Prop 6: whatever op a transaction holds (any ids, any times, any mode), `apply_tx_mode`
//!   never panics, and a success keeps the model's invariants: clips sorted with no overlap,
//!   no negative start or `sourceIn`, at least one frame long within their asset (policy modes),
//!   and every reference resolves to an asset with a stream for the clip's track.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;
use fin_core::{
    ApplyMode, AssetAdd, AudioInfo, DeleteMode, Edge, Item, ItemRemove, ItemRestore, ItemTrim,
    MemIds, Op, Project, Rational, TrackKind, TrimMode, Tx, VideoInfo, apply_internal,
    apply_tx_mode, frame_duration,
};
use proptest::prelude::*;

fn frames(n: i64) -> Rational {
    Rational::new(n, 30).unwrap()
}

fn asset_op(id: &str, secs_long: i64, video: bool) -> Op {
    AssetAdd::media(
        id,
        format!("/media/{}.mov", id.trim_start_matches('$')),
        secs(secs_long),
        video.then(|| VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
        }),
        Some(AudioInfo {
            sample_rate: 48_000,
            channels: 2,
        }),
        "fp",
    )
    .into()
}

/// `a1` (100 s, video and audio), `a2` (40 s, audio only), back-to-back clips (lengths in
/// frames) on `t1` (video, of `a1`) and `t2` (audio, of `a2`), each 1 s into its asset.
fn base(lens: &[Vec<i64>]) -> Project {
    let mut ops = vec![asset_op("$a", 100, true), asset_op("$b", 40, false)];
    for (t, track) in lens.iter().enumerate() {
        let mut at = 0;
        for (i, n) in track.iter().enumerate() {
            let asset = if t == 0 { "$a" } else { "$b" };
            let track = if t == 0 { "t1" } else { "t2" };
            let id = format!("$c{t}_{i}");
            ops.push(insert(&id, track, frames(at), asset, secs(1), frames(*n)));
            at += n;
        }
    }
    apply(&project(), ops).project
}

fn lens() -> impl Strategy<Value = Vec<Vec<i64>>> {
    prop::collection::vec(prop::collection::vec(5i64..90, 0..4), 2)
}

// ---------------------------------------------------------------------------------------------
// Prop 1: apply then inverse is exact.

/// Where an insert or a restore goes on its track: a clip boundary by index (starts first, then
/// the track end), or any frame.
#[derive(Debug, Clone)]
enum At {
    Boundary(usize),
    Frame(i64),
}

/// A generated op, realised against the current state (clips and assets picked by index).
#[derive(Debug, Clone)]
enum Gen {
    AddAsset {
        video: bool,
        secs: i64,
    },
    RemoveAsset {
        k: usize,
    },
    Insert {
        track: usize,
        at: At,
        asset: usize,
        src: i64,
        len: i64,
    },
    Trim {
        track: usize,
        k: usize,
        start: bool,
        frames: i64,
    },
    Delete {
        track: usize,
        k: usize,
    },
    Remove {
        track: usize,
        k: usize,
        ripple: bool,
    },
    /// A clip taken out earlier (`k` into the graveyard), under its own id or a fresh one
    /// (`$` placeholder), at its old start or at `at`.
    Restore {
        k: usize,
        fresh: bool,
        ripple: bool,
        at: Option<At>,
    },
}

fn at() -> impl Strategy<Value = At> {
    prop_oneof![
        3 => (0usize..8).prop_map(At::Boundary),
        1 => (0i64..400).prop_map(At::Frame),
    ]
}

fn gen_op() -> impl Strategy<Value = Gen> {
    prop_oneof![
        1 => (any::<bool>(), 1i64..60).prop_map(|(video, secs)| Gen::AddAsset { video, secs }),
        1 => (0usize..4).prop_map(|k| Gen::RemoveAsset { k }),
        1 => (0usize..2, at(), 0usize..4, 0i64..300, 1i64..120).prop_map(
            |(track, at, asset, src, len)| Gen::Insert {
                track,
                at,
                asset,
                src,
                len
            }
        ),
        1 => (0usize..2, 0usize..6, any::<bool>(), -60i64..60).prop_map(
            |(track, k, start, frames)| Gen::Trim {
                track,
                k,
                start,
                frames
            }
        ),
        1 => (0usize..2, 0usize..6).prop_map(|(track, k)| Gen::Delete { track, k }),
        1 => (0usize..2, 0usize..6, any::<bool>()).prop_map(|(track, k, ripple)| Gen::Remove {
            track,
            k,
            ripple
        }),
        2 => (0usize..8, any::<bool>(), any::<bool>(), prop::option::of(at())).prop_map(
            |(k, fresh, ripple, at)| Gen::Restore {
                k,
                fresh,
                ripple,
                at
            }
        ),
    ]
}

/// Realises generated ops one by one on an evolving state, keeping those that apply. Ids come
/// from one allocator for the whole transaction, so applying the kept ops as one transaction
/// with the same allocator binds the same ids.
struct Realiser {
    state: Project,
    ids: MemIds,
    ops: Vec<Op>,
    /// Clips taken out so far (by this or earlier transactions), with their track.
    graveyard: Vec<(String, Item)>,
    n: usize,
}

impl Realiser {
    fn position(p: &Project, track: usize, at: &At) -> Rational {
        let items = &p.timelines[0].tracks[track].items;
        match *at {
            At::Boundary(k) => items.get(k).map_or_else(
                || {
                    items
                        .last()
                        .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap())
                },
                |c| c.start,
            ),
            At::Frame(f) => frames(f),
        }
    }

    fn clip(p: &Project, track: usize, k: usize) -> Option<String> {
        let items = &p.timelines[0].tracks[track].items;
        Some(items.get(k % items.len().max(1))?.id.clone())
    }

    fn op(&mut self, g: &Gen) -> Option<Op> {
        let p = &self.state;
        self.n += 1;
        let name = format!("$n{}", self.n);
        Some(match g {
            Gen::AddAsset { video, secs } => asset_op(&name, *secs, *video),
            Gen::RemoveAsset { k } => {
                let ids: Vec<&String> = p.assets.keys().collect();
                Op::AssetRemove {
                    id: ids.get(k % ids.len().max(1))?.to_string(),
                }
            }
            Gen::Insert {
                track,
                at,
                asset,
                src,
                len,
            } => {
                let ids: Vec<&String> = p.assets.keys().collect();
                insert(
                    &name,
                    &p.timelines[0].tracks[*track].id,
                    Self::position(p, *track, at),
                    ids.get(asset % ids.len().max(1))?,
                    frames(*src),
                    frames(*len),
                )
            }
            Gen::Trim {
                track,
                k,
                start,
                frames: n,
            } => trim(
                &Self::clip(p, *track, *k)?,
                if *start { Edge::Start } else { Edge::End },
                frames(*n),
            ),
            Gen::Delete { track, k } => delete(&Self::clip(p, *track, *k)?),
            Gen::Remove { track, k, ripple } => ItemRemove {
                id: Self::clip(p, *track, *k)?,
                mode: mode_of(*ripple),
            }
            .into(),
            Gen::Restore {
                k,
                fresh,
                ripple,
                at,
            } => {
                let (track, clip) = self.graveyard.get(k % self.graveyard.len().max(1))?;
                let mut clip = clip.clone();
                if *fresh {
                    clip.id = name;
                }
                if let Some(at) = at {
                    let t = p.timelines[0].tracks.iter().position(|t| t.id == *track)?;
                    clip.start = Self::position(p, t, at);
                }
                ItemRestore {
                    item: clip,
                    track: track.clone(),
                    mode: mode_of(*ripple),
                }
                .into()
            }
        })
    }

    /// Applies `g`'s op to the state when it applies there (its mode: `Internal` for an
    /// internal op), keeping it.
    fn realise(&mut self, g: &Gen) {
        let Some(op) = self.op(g) else { return };
        let mode = if op.is_internal() {
            ApplyMode::Internal
        } else {
            ApplyMode::Client
        };
        let mut ids = self.ids.clone();
        let Ok(a) = apply_tx_mode(&self.state, &tx(vec![op.clone()]), &mut ids, mode) else {
            return;
        };
        let after: BTreeSet<String> = clips(&a.project).map(|(_, c)| c.id).collect();
        for (t, c) in clips(&self.state) {
            if !after.contains(c.id.as_str()) {
                self.graveyard.push((t.to_string(), c.clone()));
            }
        }
        self.state = a.project;
        self.ids = ids;
        self.ops.push(op);
    }
}

fn clips(p: &Project) -> impl Iterator<Item = (&str, Item)> {
    p.timelines[0]
        .tracks
        .iter()
        .flat_map(|t| t.items.iter().map(move |c| (t.id.as_str(), c.clone())))
}

fn mode_of(ripple: bool) -> DeleteMode {
    if ripple {
        DeleteMode::Ripple
    } else {
        DeleteMode::Lift
    }
}

fn internal(ops: Vec<Op>) -> Tx {
    tx(ops)
}

proptest! {
    #![proptest_config(prop_config(256))]
    /// Prop 1, over a chain of transactions of one to four ops each.
    #[test]
    fn apply_then_inverse_is_exact_for_every_op_kind(
        lens in lens(),
        txs in prop::collection::vec(prop::collection::vec(gen_op(), 1..5), 1..7),
    ) {
        let mut p = base(&lens);
        let mut graveyard = Vec::new();
        for gens in &txs {
            let mut r = Realiser {
                state: p.clone(),
                ids: MemIds::after(&p),
                ops: Vec::new(),
                graveyard,
                n: 0,
            };
            for g in gens {
                r.realise(g);
            }
            graveyard = std::mem::take(&mut r.graveyard);
            if r.ops.is_empty() {
                continue;
            }
            let mode = if r.ops.iter().any(Op::is_internal) {
                ApplyMode::Internal
            } else {
                ApplyMode::Client
            };
            let a = apply_tx_mode(&p, &tx(r.ops.clone()), &mut MemIds::after(&p), mode)
                .unwrap_or_else(|e| panic!("{:?}: {e:?}", r.ops));
            prop_assert_eq!(&a.project, &r.state, "the tx is its ops in a row");
            assert_well_formed(&a.project);
            let back = apply_internal(&a.project, &internal(a.inverse.clone()), &mut MemIds::after(&a.project))
                .unwrap_or_else(|e| panic!("inverse {:?}: {e:?}", a.inverse));
            prop_assert_eq!(&back.project, &p, "inverse of {:?}", r.ops);
            let again = apply_internal(&p, &internal(back.inverse.clone()), &mut MemIds::after(&p))
                .unwrap_or_else(|e| panic!("redo {:?}: {e:?}", back.inverse));
            prop_assert_eq!(&again.project, &a.project, "redo of {:?}", r.ops);
            p = a.project;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Prop 6: validation soundness.

/// Ids from every namespace, existing and not, placeholders and junk.
fn any_id() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "c1", "c2", "c3", "c4", "c5", "c9", "a1", "a2", "a9", "t1", "t2", "t9", "$x", "$y", "$",
        "", "x", "c0", "C1",
    ])
    .prop_map(String::from)
}

fn any_time() -> impl Strategy<Value = Rational> {
    prop_oneof![
        6 => (-120i64..400).prop_map(frames),
        2 => (-5i64..5, prop::sample::select(vec![1i64, 2, 7, 1001, 48_000]))
            .prop_map(|(n, d)| Rational::new(n, d).unwrap()),
        1 => prop::sample::select(vec![i64::MAX, i64::MIN + 1, i64::MAX / 3, 1 << 40])
            .prop_map(Rational::from_int),
        1 => (prop::sample::select(vec![i64::MAX, i64::MIN + 1]), 1i64..1_000_000)
            .prop_map(|(n, d)| Rational::new(n, d).unwrap()),
    ]
}

fn any_clip() -> impl Strategy<Value = Item> {
    (any_id(), any_id(), any_time(), any_time(), any_time()).prop_map(
        |(id, asset, start, source_in, duration)| Item::clip(id, asset, start, source_in, duration),
    )
}

fn any_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (any_id(), any_time(), any::<bool>(), any::<bool>()).prop_map(|(id, duration, v, a)| {
            AssetAdd::media(
                id,
                "/media/x.mov",
                duration,
                v.then(|| VideoInfo {
                    width: 1280,
                    height: 720,
                    rate: secs(25),
                }),
                a.then_some(AudioInfo {
                    sample_rate: 44_100,
                    channels: 1,
                }),
                "fp",
            )
            .into()
        }),
        (
            any_id(),
            any_id(),
            any_time(),
            any_id(),
            any_time(),
            any_time()
        )
            .prop_map(
                |(id, track, at, asset, source_in, duration)| Op::ClipInsert {
                    id,
                    track,
                    at,
                    asset,
                    source_in,
                    duration
                }
            ),
        (any_id(), any::<bool>(), any_time()).prop_map(|(clip, start, delta)| Op::ClipTrim {
            clip,
            edge: if start { Edge::Start } else { Edge::End },
            delta
        }),
        any_id().prop_map(|clip| Op::ClipDelete { clip }),
        any_id().prop_map(|id| Op::AssetRemove { id }),
        (any_id(), any_id(), any_time(), any_id(), any_time()).prop_map(
            |(id, track, at, asset, duration)| insert(
                &id,
                &track,
                at,
                &asset,
                Rational::zero(),
                duration
            )
        ),
        (any_id(), any::<bool>(), any_time(), any::<bool>()).prop_map(
            |(item, start, delta, strict)| {
                ItemTrim {
                    item,
                    edge: if start { Edge::Start } else { Edge::End },
                    delta,
                    mode: if strict {
                        TrimMode::Strict
                    } else {
                        TrimMode::Ripple
                    },
                }
                .into()
            }
        ),
        (any_id(), any::<bool>()).prop_map(|(id, ripple)| ItemRemove {
            id,
            mode: mode_of(ripple)
        }
        .into()),
        (any_clip(), any_id(), any::<bool>()).prop_map(|(item, track, ripple)| {
            ItemRestore {
                item,
                track,
                mode: mode_of(ripple),
            }
            .into()
        }),
    ]
}

fn any_mode() -> impl Strategy<Value = ApplyMode> {
    prop_oneof![
        2 => Just(ApplyMode::Client),
        1 => Just(ApplyMode::Internal),
        1 => Just(ApplyMode::Replay),
    ]
}

/// The model invariants (prop 6): structural ones always, policy ones unless `mode` is
/// `Replay` (which skips policy by design).
fn assert_invariants(p: &Project, mode: ApplyMode) -> Result<(), TestCaseError> {
    let frame = frame_duration(p.rate).unwrap();
    let mut seen = BTreeSet::new();
    for a in p.assets.values() {
        prop_assert!(a.duration.unwrap().is_positive(), "asset {:?}", a);
    }
    for t in &p.timelines[0].tracks {
        for c in t.items.iter().map(|c| clip(p, &c.id)) {
            prop_assert!(seen.insert(c.id.clone()), "clip id {} twice", c.id);
            prop_assert!(c.start >= Rational::zero(), "negative start {:?}", c);
            prop_assert!(c.source_in >= Rational::zero(), "negative sourceIn {:?}", c);
            prop_assert!(c.duration.is_positive(), "empty clip {:?}", c);
            let asset = p.assets.get(&c.asset);
            prop_assert!(
                asset.is_some(),
                "clip {} names missing asset {}",
                c.id,
                c.asset
            );
            let asset = asset.unwrap();
            let fits = match t.kind {
                TrackKind::Video => asset.video.is_some(),
                TrackKind::Audio => asset.audio.is_some(),
                TrackKind::Caption => false,
            };
            prop_assert!(fits, "clip {} of {} on {:?} track", c.id, c.asset, t.kind);
            if mode != ApplyMode::Replay {
                prop_assert!(c.duration >= frame, "shorter than a frame {:?}", c);
                prop_assert!(
                    (c.source_in + c.duration).unwrap() <= asset.duration.unwrap(),
                    "past its asset's end {:?}",
                    c
                );
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
    }
    Ok(())
}

proptest! {
    #![proptest_config(prop_config(512))]
    /// Prop 6: any ops, in any mode, against a well-formed project.
    #[test]
    fn apply_tx_never_panics_and_success_keeps_invariants(
        lens in lens(),
        ops in prop::collection::vec(any_op(), 1..4),
        mode in any_mode(),
    ) {
        let p = base(&lens);
        let before = p.clone();
        let mut ids = MemIds::after(&p);
        match apply_tx_mode(&p, &tx(ops.clone()), &mut ids, mode) {
            Ok(a) => {
                assert_invariants(&a.project, mode)?;
                // Bindings name each placeholder once, to ids the project now holds.
                let ids: BTreeSet<&String> = a.bindings.values().collect();
                prop_assert_eq!(ids.len(), a.bindings.len());
            }
            Err(e) => {
                prop_assert!(!e.code.is_empty());
                prop_assert!(e.details.get("opIndex").is_some() || e.code == "SCHEMA_VERSION", "{:?}", e);
            }
        }
        prop_assert_eq!(p, before, "the input is untouched");
    }
}

/// The generator above reaches every op kind (a smoke check of prop 1's coverage, with a fixed
/// seed): each kind is applied at least once.
#[test]
fn prop_1_generator_reaches_every_op_kind() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{RngAlgorithm, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        prop_config(1),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[7; 32]),
    );
    let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut fresh_restores = 0;
    for _ in 0..200 {
        let lens = lens().new_tree(&mut runner).unwrap().current();
        let gens = prop::collection::vec(gen_op(), 1..12)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let p = base(&lens);
        let mut r = Realiser {
            ids: MemIds::after(&p),
            state: p,
            ops: Vec::new(),
            graveyard: Vec::new(),
            n: 0,
        };
        for g in &gens {
            r.realise(g);
        }
        for op in &r.ops {
            *kinds.entry(op.name()).or_default() += 1;
            if let Op::ItemRestore(r) = op
                && r.item.id.starts_with('$')
            {
                fresh_restores += 1;
            }
        }
    }
    assert_eq!(kinds.len(), 7, "{kinds:?}");
    assert!(fresh_restores > 0);
}
