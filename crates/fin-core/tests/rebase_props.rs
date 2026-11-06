//! Rebase properties (spec §4.6, props 2 and 3; plan 4B Task 20) over an anchor model on three
//! tracks.
//!
//! An intention is "insert before clip k on track t", "insert at the end of track t", "trim
//! clip k's edge e by d" or "delete clip k". Clips are named by identity: a base clip by its
//! id, a clip an earlier op of the same tx created by its placeholder. An intention is realised
//! as an op against a state: at the base (the stale op) and at the head (the fresh op). A
//! position at the base is a clip start or a track end, so ties at one point resolve as
//! "before the clip that was at p".
//!
//! Intentions also add assets (an insert may use the newest one its own tx added), and the
//! committed side is sometimes a record followed by its undo (a mirrored pair, Decision 24).

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::rebase::reasons;
use fin_core::{
    CommittedTx, Edge, Link, MemIds, Op, Project, Rational, Track, TrackKind, Tx, apply_internal,
    apply_tx, rebase_tx,
};
use proptest::prelude::*;

const TRACKS: [&str; 3] = ["t1", "t2", "t3"];

fn frames(n: i64) -> Rational {
    Rational::new(n, 30).unwrap()
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

/// Asset `a1` (100 s, video and audio) and back-to-back clips (lengths in frames) on `t1`
/// (video), `t2` and `t3` (audio), each starting 10 s into the asset.
fn base(lens: &[Vec<i64>]) -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Audio, "A2"));
    let mut ops = vec![av_asset("$a", secs(100))];
    for (t, track) in lens.iter().enumerate() {
        let mut at = 0;
        for (i, n) in track.iter().enumerate() {
            let name = format!("$c{t}_{i}");
            ops.push(insert(
                &name,
                TRACKS[t],
                frames(at),
                "$a",
                secs(10),
                frames(*n),
            ));
            at += n;
        }
    }
    apply(&p, ops).project
}

/// A generated intention: clips are picked by index on the track when realised at the base.
#[derive(Debug, Clone)]
enum Intent {
    /// `own_asset`: the clip uses the newest asset this tx added (else `a1`).
    Insert {
        track: usize,
        before: Option<usize>,
        len: i64,
        own_asset: bool,
    },
    Asset,
    Trim {
        track: usize,
        clip: usize,
        start: bool,
        frames: i64,
    },
    Delete {
        track: usize,
        clip: usize,
    },
}

fn intent() -> impl Strategy<Value = Intent> {
    prop_oneof![
        (
            0usize..3,
            prop::option::of(0usize..6),
            1i64..60,
            any::<bool>()
        )
            .prop_map(|(track, before, len, own_asset)| Intent::Insert {
                track,
                before,
                len,
                own_asset
            }),
        Just(Intent::Asset),
        (0usize..3, 0usize..6, any::<bool>(), -12i64..12)
            .prop_filter("a trim moves", |t| t.3 != 0)
            .prop_map(|(track, clip, start, frames)| Intent::Trim {
                track,
                clip,
                start,
                frames
            }),
        (0usize..3, 0usize..6).prop_map(|(track, clip)| Intent::Delete { track, clip }),
    ]
}

/// A clip by identity: a literal id, or the placeholder of a clip our own tx created.
#[derive(Debug, Clone, PartialEq)]
enum Key {
    Id(String),
    Own(String),
}

/// An intention with its clips named by identity.
#[derive(Debug, Clone)]
enum Keyed {
    Insert {
        name: String,
        track: &'static str,
        before: Option<Key>,
        len: i64,
        /// The placeholder of an asset this tx added, else `a1`.
        asset: Option<String>,
    },
    Asset {
        name: String,
    },
    Trim {
        clip: Key,
        edge: Edge,
        frames: i64,
    },
    Delete {
        clip: Key,
    },
}

/// Realises intentions one by one against an evolving state.
struct Realiser {
    state: Project,
    ids: MemIds,
    /// Placeholder → id, for the clips our earlier ops created in this realisation.
    bound: BTreeMap<String, String>,
    /// Placeholders of the assets our earlier ops added, oldest first.
    assets: Vec<String>,
    ops: Vec<Op>,
}

impl Realiser {
    fn new(state: &Project) -> Realiser {
        Realiser::with_ids(state, MemIds::after(state))
    }

    /// `ids` must be past every id ever allocated, so a new clip never takes the id of a base
    /// clip deleted since (which a `Key::Id` would then name).
    fn with_ids(state: &Project, ids: MemIds) -> Realiser {
        Realiser {
            state: state.clone(),
            ids,
            bound: BTreeMap::new(),
            assets: Vec::new(),
            ops: Vec::new(),
        }
    }

    /// The id `key` names in the current state, if that clip is there.
    fn id(&self, key: &Key) -> Option<String> {
        let id = match key {
            Key::Id(id) => id.clone(),
            Key::Own(p) => self.bound.get(p)?.clone(),
        };
        let tracks = &self.state.timelines[0].tracks;
        let present = tracks.iter().flat_map(|t| &t.items).any(|c| c.id == id);
        present.then_some(id)
    }

    /// How an op refers to `key`: its id, or its placeholder for our own clips.
    fn reference(key: &Key) -> String {
        match key {
            Key::Id(id) | Key::Own(id) => id.clone(),
        }
    }

    /// The op for `k` in the current state, applied to it. `None` when its clip is gone or
    /// the op does not apply here.
    fn realise(&mut self, k: &Keyed) -> Option<()> {
        let op = match k {
            Keyed::Insert {
                name,
                track,
                before,
                len,
                asset,
            } => {
                let t = self.state.timelines[0]
                    .tracks
                    .iter()
                    .find(|t| t.id == *track)?;
                let at = match before {
                    Some(key) => {
                        let id = self.id(key)?;
                        t.items.iter().find(|c| c.id == id)?.start
                    }
                    None => t
                        .items
                        .last()
                        .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap()),
                };
                let asset = asset.as_deref().unwrap_or("a1");
                insert(name, track, at, asset, secs(10), frames(*len))
            }
            Keyed::Asset { name } => av_asset(name, secs(100)),
            Keyed::Trim {
                clip,
                edge,
                frames: n,
            } => {
                self.id(clip)?;
                Op::ClipTrim {
                    clip: Self::reference(clip),
                    edge: *edge,
                    delta: frames(*n),
                }
            }
            Keyed::Delete { clip } => {
                self.id(clip)?;
                Op::ClipDelete {
                    clip: Self::reference(clip),
                }
            }
        };
        let mut resolved = op.clone();
        if let Op::ClipTrim { clip: r, .. }
        | Op::ClipDelete { clip: r }
        | Op::ClipInsert { asset: r, .. } = &mut resolved
            && let Some(id) = self.bound.get(r.as_str())
        {
            *r = id.clone();
        }
        let a = apply_tx(&self.state, &tx(vec![resolved]), &mut self.ids).ok()?;
        self.bound.extend(a.bindings);
        if let Keyed::Asset { name } = k {
            self.assets.push(name.clone());
        }
        self.state = a.project;
        self.ops.push(op);
        Some(())
    }

    /// Keys `intents` against the current state and realises each; intentions that do not
    /// apply are dropped.
    fn keyed(&mut self, intents: &[Intent], prefix: &str) -> Vec<Keyed> {
        let mut out = Vec::new();
        for (i, intent) in intents.iter().enumerate() {
            let pick = |r: &Realiser, track: usize, k: usize| {
                let items = &r.state.timelines[0].tracks[track].items;
                let id = &items.get(k % items.len().max(1))?.id;
                let own = r.bound.iter().find(|(_, v)| *v == id).map(|(p, _)| p);
                Some(own.map_or_else(|| Key::Id(id.clone()), |p| Key::Own(p.clone())))
            };
            let k = match *intent {
                Intent::Asset => Keyed::Asset {
                    name: format!("${prefix}{i}"),
                },
                Intent::Insert {
                    track,
                    before,
                    len,
                    own_asset,
                } => Keyed::Insert {
                    asset: own_asset.then(|| self.assets.last().cloned()).flatten(),
                    name: format!("${prefix}{i}"),
                    track: TRACKS[track],
                    before: match before {
                        Some(k) => match pick(self, track, k) {
                            Some(key) => Some(key),
                            None => continue,
                        },
                        None => None,
                    },
                    len,
                },
                Intent::Trim {
                    track,
                    clip,
                    start,
                    frames,
                } => {
                    let Some(clip) = pick(self, track, clip) else {
                        continue;
                    };
                    let edge = if start { Edge::Start } else { Edge::End };
                    Keyed::Trim { clip, edge, frames }
                }
                Intent::Delete { track, clip } => {
                    let Some(clip) = pick(self, track, clip) else {
                        continue;
                    };
                    Keyed::Delete { clip }
                }
            };
            if self.realise(&k).is_some() {
                out.push(k);
            }
        }
        out
    }
}

/// `intents` realised at `p` as one committed record at `rev`, with ids from `ids`; also its
/// inverse ops.
fn commit(
    p: &Project,
    intents: &[Intent],
    rev: u64,
    ids: &mut MemIds,
) -> Option<(Project, CommittedTx, Vec<Op>)> {
    let mut r = Realiser::new(p);
    r.keyed(intents, &format!("t{rev}_"));
    if r.ops.is_empty() {
        return None;
    }
    let mut t = tx(r.ops);
    t.tx_id = format!("tx-{rev}");
    t.base_rev = rev - 1;
    let a = apply_tx(p, &t, ids).unwrap();
    let c = CommittedTx {
        rev,
        tx: t,
        diff: a.diff,
        bindings: a.bindings,
        link: None,
    };
    Some((a.project, c, a.inverse))
}

/// The undo of `unit` (its stored inverse, applied at `p`) as the record at `rev`.
fn undo(p: &Project, unit: &CommittedTx, inverse: Vec<Op>, rev: u64) -> (Project, CommittedTx) {
    let mut t = tx(inverse);
    t.tx_id = format!("tx-{rev}");
    t.base_rev = rev - 1;
    let a = apply_internal(p, &t, &mut MemIds::after(p)).unwrap();
    let c = CommittedTx {
        rev,
        tx: t,
        diff: a.diff,
        bindings: a.bindings,
        link: Some(Box::new(Link::Undo {
            undoes: vec![unit.tx.tx_id.clone()],
        })),
    };
    (a.project, c)
}

fn ours(ops: Vec<Op>) -> Tx {
    let mut t = tx(ops);
    t.tx_id = "tx-ours".into();
    t
}

fn lens() -> impl Strategy<Value = Vec<Vec<i64>>> {
    prop::collection::vec(prop::collection::vec(10i64..60, 0..4), 3)
}

proptest! {
    #![proptest_config(config(256))]
    /// Prop 2: rebasing over nothing is applying at the head.
    #[test]
    fn rebase_over_empty_interval_is_identity(
        lens in lens(),
        intents in prop::collection::vec(intent(), 1..5),
    ) {
        let p = base(&lens);
        let mut r = Realiser::new(&p);
        r.keyed(&intents, "o");
        if r.ops.is_empty() {
            return Ok(());
        }
        let t = ours(r.ops);
        let ids = MemIds::after(&p);
        let fresh = apply_tx(&p, &t, &mut ids.clone()).unwrap();
        let rb = rebase_tx(&p, &p, &t, &[], &mut ids.clone()).unwrap();
        prop_assert_eq!(rb.tx, t);
        prop_assert_eq!(rb.applied, fresh);
    }
}

proptest! {
    #![proptest_config(config(128))]
    /// Prop 3: when the stale rebase and the fresh realisation at the head both succeed, they
    /// produce the same state. The interval holds 0–6 committed edits, each sometimes followed
    /// by its undo (a mirrored pair).
    #[test]
    fn stale_and_fresh_agree_multi_track(
        lens in lens(),
        committed in prop::collection::vec(
            (prop::collection::vec(intent(), 1..3), any::<bool>()),
            0..7,
        ),
        mine in prop::collection::vec(intent(), 1..4),
    ) {
        let p = base(&lens);
        let mut ids = MemIds::after(&p);
        let mut head = p.clone();
        let mut since: Vec<CommittedTx> = Vec::new();
        for (intents, undo_it) in &committed {
            let rev = since.len() as u64 + 1;
            let Some((next, c, inverse)) = commit(&head, intents, rev, &mut ids) else { continue };
            head = next;
            since.push(c);
            if *undo_it {
                let (next, u) = undo(&head, since.last().unwrap(), inverse, rev + 1);
                since.push(u);
                head = next;
            }
        }
        let mut at_base = Realiser::new(&p);
        let keyed = at_base.keyed(&mine, "o");
        if at_base.ops.is_empty() {
            return Ok(());
        }
        let mut at_head = Realiser::with_ids(&head, ids.clone());
        let fresh_ok = keyed.iter().all(|k| at_head.realise(k).is_some());
        let stale = rebase_tx(&p, &head, &ours(at_base.ops), &since, &mut ids.clone());
        match (fresh_ok, stale) {
            (true, Ok(stale)) => {
                let fresh = apply_tx(&head, &ours(at_head.ops), &mut ids.clone()).unwrap();
                assert_well_formed(&stale.applied.project);
                prop_assert_eq!(&stale.applied.project, &fresh.project, "stale {:?}", stale.tx.ops);
            }
            // Where the fresh edit applies, the stale one may refuse only by the entity rules.
            (true, Err(e)) => {
                let reason = e.details["reason"].as_str().unwrap_or(&e.code).to_string();
                prop_assert!(
                    [reasons::SAME_EDGE, reasons::REMOVED].contains(&reason.as_str()),
                    "{e:?}"
                );
            }
            (false, _) => {}
        }
    }
}
