//! Rebase properties (spec §4.6, props 2 and 3) over an anchor model on three tracks.
//!
//! An intention is "insert before clip k on track t", "insert at the end of track t", "trim
//! clip k's edge e by d" or "delete clip k". Clips are named by identity: a base clip by its
//! id, a clip an earlier op of the same tx created by its placeholder. An intention is realised
//! as an op against a state: at the base (the stale op) and at the head (the fresh op). A
//! position at the base is a clip start or a track end, so ties at one point resolve as
//! "before the clip that was at p".

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::{
    CommittedTx, Edge, MemIds, Op, Project, Rational, Track, TrackKind, Tx, apply_tx, rebase_tx,
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
    p.timeline.tracks.push(Track {
        id: "t3".into(),
        kind: TrackKind::Audio,
        items: Vec::new(),
    });
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
    Insert {
        track: usize,
        before: Option<usize>,
        len: i64,
    },
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
        (0usize..3, prop::option::of(0usize..6), 1i64..60)
            .prop_map(|(track, before, len)| { Intent::Insert { track, before, len } }),
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
            ops: Vec::new(),
        }
    }

    /// The id `key` names in the current state, if that clip is there.
    fn id(&self, key: &Key) -> Option<String> {
        let id = match key {
            Key::Id(id) => id.clone(),
            Key::Own(p) => self.bound.get(p)?.clone(),
        };
        let tracks = &self.state.timeline.tracks;
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
            } => {
                let t = self.state.timeline.tracks.iter().find(|t| t.id == *track)?;
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
                insert(name, track, at, "a1", secs(10), frames(*len))
            }
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
        if let Op::ClipTrim { clip, .. } | Op::ClipDelete { clip } = &mut resolved
            && let Some(id) = self.bound.get(clip.as_str())
        {
            *clip = id.clone();
        }
        let a = apply_tx(&self.state, &tx(vec![resolved]), &mut self.ids).ok()?;
        self.bound.extend(a.bindings);
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
                let items = &r.state.timeline.tracks[track].items;
                let id = &items.get(k % items.len().max(1))?.id;
                let own = r.bound.iter().find(|(_, v)| *v == id).map(|(p, _)| p);
                Some(own.map_or_else(|| Key::Id(id.clone()), |p| Key::Own(p.clone())))
            };
            let k = match *intent {
                Intent::Insert { track, before, len } => Keyed::Insert {
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

/// `intents` realised at `p` as one committed record at `rev`, with ids from `ids`.
fn commit(
    p: &Project,
    intents: &[Intent],
    rev: u64,
    ids: &mut MemIds,
) -> Option<(Project, CommittedTx)> {
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
    Some((a.project, c))
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
        prop_assume!(!r.ops.is_empty());
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
    /// produce the same state.
    #[test]
    fn stale_and_fresh_agree_multi_track(
        lens in lens(),
        first in prop::collection::vec(intent(), 1..3),
        second in prop::collection::vec(intent(), 0..3),
        mine in prop::collection::vec(intent(), 1..4),
    ) {
        let p = base(&lens);
        let mut ids = MemIds::after(&p);
        let Some((p1, c1)) = commit(&p, &first, 1, &mut ids) else { return Ok(()) };
        let mut since = vec![c1];
        let head = match commit(&p1, &second, 2, &mut ids) {
            Some((p2, c2)) => {
                since.push(c2);
                p2
            }
            None => p1,
        };
        let mut at_base = Realiser::new(&p);
        let keyed = at_base.keyed(&mine, "o");
        prop_assume!(!at_base.ops.is_empty());
        let mut at_head = Realiser::with_ids(&head, ids.clone());
        let fresh_ok = keyed.iter().all(|k| at_head.realise(k).is_some());
        let stale = rebase_tx(&p, &head, &ours(at_base.ops), &since, &mut ids.clone());
        if let (true, Ok(stale)) = (fresh_ok, stale) {
            let fresh = apply_tx(&head, &ours(at_head.ops), &mut ids.clone()).unwrap();
            assert_well_formed(&stale.applied.project);
            prop_assert_eq!(&stale.applied.project, &fresh.project, "stale {:?}", stale.tx.ops);
        }
    }
}
