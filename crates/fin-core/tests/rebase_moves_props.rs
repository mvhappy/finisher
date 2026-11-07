//! Differential stale-vs-fresh property for move, slide, link and transition edits (4C Tasks
//! 9–10 review; extends 4B's `rebase_props` intent model; input for Task 26): our intents
//! realised at the base and rebased over committed ones (some undone) agree with the same
//! intents realised at the head, or conflict; committed records replay exactly and undo
//! exactly; a successful stale rebase replays and undoes exactly and keeps transitions at cuts.

mod common;

use std::collections::BTreeMap;

use common::moves::{link, move_to, slide, unlink};
use common::*;
use fin_core::{
    CommittedTx, Curve, Edge, InsertMode, Link, MemIds, Op, Project, Rational, Track, TrackKind,
    TransitionKind, TransitionRemove, TransitionSet, apply_internal, apply_tx, rebase_tx,
};
use proptest::prelude::*;

const TRACKS: [&str; 4] = ["t1", "t2", "t3", "t4"];

fn frames(n: i64) -> Rational {
    Rational::new(n, 30).unwrap()
}

fn kind_of(track: &str) -> TransitionKind {
    if track == "t1" || track == "t4" {
        TransitionKind::Dissolve
    } else {
        TransitionKind::Crossfade
    }
}

/// t1 V1, t2 A1, t3 A2, t4 V2; back-to-back clips; optionally c0_0 linked with c1_0, and a
/// transition at every cut.
fn base(lens: &[Vec<i64>], linked: bool, trs: bool) -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Audio, "A2"));
    p.timelines[0]
        .tracks
        .push(Track::new("t4", TrackKind::Video, "V2"));
    let mut ops = vec![av_asset("$a", secs(100))];
    for (t, track) in lens.iter().enumerate() {
        let mut at = 0;
        for (i, n) in track.iter().enumerate() {
            ops.push(insert(
                &format!("$c{t}_{i}"),
                TRACKS[t],
                frames(at),
                "$a",
                secs(10),
                frames(*n),
            ));
            if trs && i > 0 {
                ops.push(
                    TransitionSet {
                        id: format!("$tr{t}_{i}"),
                        track: TRACKS[t].into(),
                        between: [format!("$c{t}_{}", i - 1), format!("$c{t}_{i}")],
                        kind: kind_of(TRACKS[t]),
                        duration: frames(4),
                        curve: Curve::Linear,
                    }
                    .into(),
                );
            }
            at += n;
        }
    }
    if linked && !lens[0].is_empty() && !lens[1].is_empty() {
        ops.push(link(&["$c0_0", "$c1_0"], "$lk0"));
    }
    apply(&p, ops).project
}

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
    Move {
        track: usize,
        clip: usize,
        to: usize,
        before: Option<usize>,
        overwrite: bool,
    },
    Slide {
        track: usize,
        clip: usize,
        frames: i64,
    },
    Link {
        ta: usize,
        a: usize,
        tb: usize,
        b: usize,
    },
    Unlink {
        track: usize,
        clip: usize,
    },
    TrSet {
        track: usize,
        clip: usize,
    },
    TrRemove {
        track: usize,
        k: usize,
    },
    TrUpdate {
        track: usize,
        k: usize,
        frames: i64,
    },
}

fn intent() -> impl Strategy<Value = Intent> {
    let t = 0usize..4;
    let c = 0usize..6;
    prop_oneof![
        1 => (t.clone(), prop::option::of(c.clone()), 1i64..40)
            .prop_map(|(track, before, len)| Intent::Insert { track, before, len }),
        1 => (t.clone(), c.clone(), any::<bool>(), -8i64..8)
            .prop_filter("moves", |x| x.3 != 0)
            .prop_map(|(track, clip, start, frames)| Intent::Trim { track, clip, start, frames }),
        1 => (t.clone(), c.clone()).prop_map(|(track, clip)| Intent::Delete { track, clip }),
        3 => (t.clone(), c.clone(), t.clone(), prop::option::of(c.clone()), any::<bool>())
            .prop_map(|(track, clip, to, before, overwrite)| Intent::Move {
                track, clip, to, before, overwrite
            }),
        2 => (t.clone(), c.clone(), -6i64..6)
            .prop_filter("moves", |x| x.2 != 0)
            .prop_map(|(track, clip, frames)| Intent::Slide { track, clip, frames }),
        1 => (t.clone(), c.clone(), t.clone(), c.clone())
            .prop_map(|(ta, a, tb, b)| Intent::Link { ta, a, tb, b }),
        1 => (t.clone(), c.clone()).prop_map(|(track, clip)| Intent::Unlink { track, clip }),
        2 => (t.clone(), c.clone()).prop_map(|(track, clip)| Intent::TrSet { track, clip }),
        1 => (t.clone(), c.clone()).prop_map(|(track, k)| Intent::TrRemove { track, k }),
        1 => (t.clone(), c.clone(), 1i64..8)
            .prop_map(|(track, k, frames)| Intent::TrUpdate { track, k, frames }),
    ]
}

#[derive(Debug, Clone, PartialEq)]
enum Key {
    Id(String),
    Own(String),
}

#[derive(Debug, Clone)]
enum Anchor {
    Before(Key),
    After(Key),
    Zero,
}

#[derive(Debug, Clone)]
enum Keyed {
    Insert {
        name: String,
        track: &'static str,
        before: Anchor,
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
    Move {
        clip: Key,
        to: &'static str,
        before: Anchor,
        overwrite: bool,
    },
    Slide {
        clip: Key,
        frames: i64,
    },
    Link {
        name: String,
        a: Key,
        b: Key,
    },
    Unlink {
        group: Key,
    },
    TrSet {
        name: String,
        track: &'static str,
        a: Key,
        b: Key,
    },
    TrRemove {
        tr: Key,
    },
    TrUpdate {
        tr: Key,
        frames: i64,
    },
}

struct Realiser {
    state: Project,
    ids: MemIds,
    bound: BTreeMap<String, String>,
    ops: Vec<Op>,
    /// At the head: the item ids of the base (items not in it were inserted concurrently).
    base: Option<std::collections::BTreeSet<String>>,
}

fn reference(key: &Key) -> String {
    match key {
        Key::Id(id) | Key::Own(id) => id.clone(),
    }
}

impl Realiser {
    fn with_ids(state: &Project, ids: MemIds) -> Realiser {
        Realiser {
            state: state.clone(),
            ids,
            bound: BTreeMap::new(),
            ops: Vec::new(),
            base: None,
        }
    }

    fn resolve(&self, key: &Key) -> Option<String> {
        Some(match key {
            Key::Id(id) => id.clone(),
            Key::Own(p) => self.bound.get(p)?.clone(),
        })
    }

    /// (track index, item index) of the item `key` names.
    fn item(&self, key: &Key) -> Option<(usize, usize)> {
        let id = self.resolve(key)?;
        let tracks = &self.state.timelines[0].tracks;
        tracks
            .iter()
            .enumerate()
            .find_map(|(t, tr)| tr.items.iter().position(|c| c.id == id).map(|i| (t, i)))
    }

    fn transition(&self, key: &Key) -> Option<(usize, usize)> {
        let id = self.resolve(key)?;
        let tracks = &self.state.timelines[0].tracks;
        tracks.iter().enumerate().find_map(|(t, tr)| {
            tr.transitions
                .iter()
                .position(|x| x.id == id)
                .map(|i| (t, i))
        })
    }

    fn key_of(&self, id: &str) -> Key {
        match self.bound.iter().find(|(_, v)| *v == id) {
            Some((p, _)) => Key::Own(p.clone()),
            None => Key::Id(id.to_string()),
        }
    }

    fn pick(&self, track: usize, k: usize) -> Option<Key> {
        let items = &self.state.timelines[0].tracks[track].items;
        let id = &items.get(k % items.len().max(1))?.id;
        Some(self.key_of(id))
    }

    fn anchor(&self, a: &Anchor, track: &str) -> Option<Rational> {
        let tracks = &self.state.timelines[0].tracks;
        match a {
            Anchor::Zero => {
                let t = tracks.iter().find(|x| x.id == track)?;
                t.items.is_empty().then(Rational::zero)
            }
            Anchor::Before(key) | Anchor::After(key) => {
                let (t, i) = self.item(key)?;
                if TRACKS[t] != track {
                    return None;
                }
                let c = &tracks[t].items[i];
                Some(match a {
                    Anchor::Before(_) => c.start,
                    _ => {
                        // Assoc::After: past items inserted concurrently at that point.
                        let mut at = (c.start + c.duration).unwrap();
                        while let Some(base) = &self.base
                            && let Some(n) = tracks[t].items.iter().find(|n| {
                                n.start == at
                                    && !base.contains(&n.id)
                                    && !self.bound.values().any(|v| *v == n.id)
                            })
                        {
                            at = (n.start + n.duration).unwrap();
                        }
                        at
                    }
                })
            }
        }
    }

    fn anchor_at(&self, track: usize, before: Option<usize>) -> Anchor {
        let items = &self.state.timelines[0].tracks[track].items;
        match before {
            _ if items.is_empty() => Anchor::Zero,
            Some(k) => Anchor::Before(self.pick(track, k).unwrap()),
            None => Anchor::After(self.key_of(&items.last().unwrap().id.clone())),
        }
    }

    fn realise(&mut self, k: &Keyed) -> Option<()> {
        let tracks = self.state.timelines[0].tracks.clone();
        let op: Op = match k {
            Keyed::Insert {
                name,
                track,
                before,
                len,
            } => {
                let at = self.anchor(before, track)?;
                insert(name, track, at, "a1", secs(10), frames(*len))
            }
            Keyed::Trim {
                clip,
                edge,
                frames: n,
            } => {
                self.item(clip)?;
                trim(&reference(clip), *edge, frames(*n))
            }
            Keyed::Delete { clip } => {
                self.item(clip)?;
                delete(&reference(clip))
            }
            Keyed::Move {
                clip,
                to,
                before,
                overwrite,
            } => {
                self.item(clip)?;
                let at = self.anchor(before, to)?;
                let mode = if *overwrite {
                    InsertMode::Overwrite
                } else {
                    InsertMode::Ripple
                };
                move_to(&reference(clip), to, at, mode)
            }
            Keyed::Slide { clip, frames: n } => {
                self.item(clip)?;
                slide(&reference(clip), frames(*n))
            }
            Keyed::Link { name, a, b } => {
                self.item(a)?;
                self.item(b)?;
                link(&[&reference(a), &reference(b)], name)
            }
            Keyed::Unlink { group } => {
                let g = self.resolve(group)?;
                let present = tracks
                    .iter()
                    .flat_map(|t| &t.items)
                    .any(|c| c.common.link.as_deref() == Some(g.as_str()));
                if !present {
                    return None;
                }
                unlink(&reference(group))
            }
            Keyed::TrSet { name, track, a, b } => {
                let (ta, ia) = self.item(a)?;
                let (tb, ib) = self.item(b)?;
                if ta != tb || TRACKS[ta] != *track || ib != ia + 1 {
                    return None;
                }
                let (x, y) = (&tracks[ta].items[ia], &tracks[ta].items[ib]);
                if (x.start + x.duration).unwrap() != y.start {
                    return None;
                }
                TransitionSet {
                    id: name.clone(),
                    track: track.to_string(),
                    between: [reference(a), reference(b)],
                    kind: kind_of(track),
                    duration: frames(3),
                    curve: Curve::Linear,
                }
                .into()
            }
            Keyed::TrRemove { tr } => {
                self.transition(tr)?;
                TransitionRemove {
                    transition: reference(tr),
                }
                .into()
            }
            Keyed::TrUpdate { tr, frames: n } => {
                let (t, i) = self.transition(tr)?;
                let x = &tracks[t].transitions[i];
                TransitionSet {
                    id: reference(tr),
                    track: TRACKS[t].into(),
                    between: [
                        reference(&self.key_of(&x.between[0])),
                        reference(&self.key_of(&x.between[1])),
                    ],
                    kind: x.kind,
                    duration: frames(*n),
                    curve: Curve::EqualPower,
                }
                .into()
            }
        };
        let mut resolved = op.clone();
        let allocated = resolved.allocates().map(str::to_string);
        for r in resolved.refs_mut() {
            if allocated.as_deref() != Some(r.as_str())
                && let Some(id) = self.bound.get(r.as_str())
            {
                *r = id.clone();
            }
        }
        let a = apply_tx(&self.state, &tx(vec![resolved]), &mut self.ids).ok()?;
        self.bound.extend(a.bindings);
        self.state = a.project;
        self.ops.push(op);
        Some(())
    }

    fn keyed(&mut self, intents: &[Intent], prefix: &str) -> Vec<Keyed> {
        let mut out = Vec::new();
        for (i, intent) in intents.iter().enumerate() {
            let name = format!("${prefix}{i}");
            let k = match *intent {
                Intent::Insert { track, before, len } => Keyed::Insert {
                    name,
                    track: TRACKS[track],
                    before: self.anchor_at(track, before),
                    len,
                },
                Intent::Trim {
                    track,
                    clip,
                    start,
                    frames,
                } => {
                    let Some(clip) = self.pick(track, clip) else {
                        continue;
                    };
                    let edge = if start { Edge::Start } else { Edge::End };
                    Keyed::Trim { clip, edge, frames }
                }
                Intent::Delete { track, clip } => {
                    let Some(clip) = self.pick(track, clip) else {
                        continue;
                    };
                    Keyed::Delete { clip }
                }
                Intent::Move {
                    track,
                    clip,
                    to,
                    before,
                    overwrite,
                } => {
                    let Some(clip) = self.pick(track, clip) else {
                        continue;
                    };
                    let before = self.anchor_at(to, before);
                    Keyed::Move {
                        clip,
                        to: TRACKS[to],
                        before,
                        overwrite,
                    }
                }
                Intent::Slide {
                    track,
                    clip,
                    frames,
                } => {
                    let Some(clip) = self.pick(track, clip) else {
                        continue;
                    };
                    Keyed::Slide { clip, frames }
                }
                Intent::Link { ta, a, tb, b } => {
                    let (Some(a), Some(b)) = (self.pick(ta, a), self.pick(tb, b)) else {
                        continue;
                    };
                    Keyed::Link { name, a, b }
                }
                Intent::Unlink { track, clip } => {
                    let Some(key) = self.pick(track, clip) else {
                        continue;
                    };
                    let (t, i) = self.item(&key).unwrap();
                    let Some(g) = self.state.timelines[0].tracks[t].items[i]
                        .common
                        .link
                        .clone()
                    else {
                        continue;
                    };
                    Keyed::Unlink {
                        group: self.key_of(&g),
                    }
                }
                Intent::TrSet { track, clip } => {
                    let items = &self.state.timelines[0].tracks[track].items;
                    if items.len() < 2 {
                        continue;
                    }
                    let i = clip % (items.len() - 1);
                    let (a, b) = (items[i].id.clone(), items[i + 1].id.clone());
                    Keyed::TrSet {
                        name,
                        track: TRACKS[track],
                        a: self.key_of(&a),
                        b: self.key_of(&b),
                    }
                }
                Intent::TrRemove { track, k } | Intent::TrUpdate { track, k, .. } => {
                    let trs = &self.state.timelines[0].tracks[track].transitions;
                    if trs.is_empty() {
                        continue;
                    }
                    let tr = self.key_of(&trs[k % trs.len()].id.clone());
                    match *intent {
                        Intent::TrRemove { .. } => Keyed::TrRemove { tr },
                        Intent::TrUpdate { frames, .. } => Keyed::TrUpdate { tr, frames },
                        _ => unreachable!(),
                    }
                }
            };
            if self.realise(&k).is_some() {
                out.push(k);
            }
        }
        out
    }
}

fn commit(
    p: &Project,
    intents: &[Intent],
    rev: u64,
    ids: &mut MemIds,
) -> Option<(Project, CommittedTx, Vec<Op>)> {
    let mut r = Realiser::with_ids(p, ids.clone());
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

fn undo(
    p: &Project,
    unit: &CommittedTx,
    inverse: Vec<Op>,
    rev: u64,
) -> Option<(Project, CommittedTx)> {
    let mut t = tx(inverse);
    t.tx_id = format!("tx-{rev}");
    t.base_rev = rev - 1;
    let a = apply_internal(p, &t, &mut MemIds::after(p)).ok()?;
    let c = CommittedTx {
        rev,
        tx: t,
        diff: a.diff,
        bindings: a.bindings,
        link: Some(Box::new(Link::Undo {
            undoes: vec![unit.tx.tx_id.clone()],
        })),
    };
    Some((a.project, c))
}

/// Transitions are at cuts of touching items (the structural invariant).
fn transitions_valid(p: &Project) -> Result<(), String> {
    for tr in &p.timelines[0].tracks {
        for x in &tr.transitions {
            let a = tr.items.iter().position(|c| c.id == x.between[0]);
            let b = tr.items.iter().position(|c| c.id == x.between[1]);
            match (a, b) {
                (Some(a), Some(b))
                    if b == a + 1
                        && (tr.items[a].start + tr.items[a].duration).unwrap()
                            == tr.items[b].start => {}
                _ => return Err(format!("{} on {} not at a cut", x.id, tr.id)),
            }
        }
    }
    Ok(())
}

/// Known limitation (4C Tasks 9–10 review, not B1): a committed `item.link` grew the group of
/// an item one of our moves names, so our rebased move also moves a partner on a track our
/// move never touched where it was written; a later op of ours anchored at that partner maps
/// over its close like any position (no mirror records where it went), not along with it.
fn group_grew(keyed: &[Keyed], since: &[CommittedTx]) -> bool {
    let joined: std::collections::BTreeSet<String> = since
        .iter()
        .flat_map(|c| &c.diff.changes)
        .filter(|ch| ch.path.as_deref() == Some("link"))
        .map(|ch| ch.id.clone())
        .collect();
    let first = keyed
        .iter()
        .position(|k| matches!(k, Keyed::Move { clip: Key::Id(id), .. } if joined.contains(id)));
    first.is_some_and(|f| {
        keyed[f + 1..]
            .iter()
            .any(|k| matches!(k, Keyed::Insert { .. } | Keyed::Move { .. }))
    })
}

/// Known limitation (4C Tasks 9–10 review): our ripple move of a linked item moves each partner
/// by the named item's delta, which the head may change (a move's `at` maps over the head's
/// edits on the named item's track only); a later op of ours anchored on another track, where
/// a partner landed, then maps by the delta the move had where it was written.
fn partner_anchor(keyed: &[Keyed], base: &Project) -> bool {
    let linked: std::collections::BTreeSet<&str> = base.timelines[0]
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .filter(|c| c.common.link.is_some())
        .map(|c| c.id.as_str())
        .collect();
    let mine_links = keyed.iter().any(|k| matches!(k, Keyed::Link { .. }));
    keyed.iter().enumerate().any(|(f, k)| {
        let Keyed::Move { clip, to, .. } = k else {
            return false;
        };
        let grouped = mine_links || matches!(clip, Key::Id(id) if linked.contains(id.as_str()));
        grouped
            && keyed[f + 1..].iter().any(|later| match later {
                Keyed::Insert { track, .. } => track != to,
                Keyed::Move { to: t, .. } => t != to,
                _ => false,
            })
    })
}

/// Known semantics (not a 4C review finding): a committed relocation without a shift (an
/// overwrite move lifts its item, a slide moves it) leaves the positions past it where they
/// were, gap included, so a stale positional op of ours on a track it touched can land past a
/// gap a fresh one closes or skips.
fn lifted_on(keyed: &[Keyed], since: &[CommittedTx]) -> bool {
    let tracks: std::collections::BTreeSet<&str> = since
        .iter()
        .flat_map(|c| c.diff.changes.iter().map(move |ch| (c, ch)))
        .filter(|(c, ch)| {
            ch.path.as_deref() == Some("start")
                && matches!(&c.tx.ops[ch.op as usize],
                    Op::ItemMove(m) if m.mode == InsertMode::Overwrite)
                || matches!(&c.tx.ops[ch.op as usize], Op::ItemSlide(_))
        })
        .filter_map(|(_, ch)| ch.track.as_deref())
        .collect();
    keyed.iter().any(|k| match k {
        Keyed::Insert { track, .. } | Keyed::Move { to: track, .. } => tracks.contains(track),
        _ => false,
    })
}

/// Our own shift-less relocation (slide, overwrite move) before a positional op.
fn self_relocates(keyed: &[Keyed]) -> bool {
    let first = keyed.iter().position(|k| {
        matches!(
            k,
            Keyed::Slide { .. }
                | Keyed::Move {
                    overwrite: true,
                    ..
                }
        )
    });
    first.is_some_and(|f| {
        keyed[f + 1..]
            .iter()
            .any(|k| matches!(k, Keyed::Insert { .. } | Keyed::Move { .. }))
    })
}

/// Ids (literal) mine's position anchors name.
fn anchor_ids(keyed: &[Keyed]) -> Vec<String> {
    let mut out = Vec::new();
    for k in keyed {
        let a = match k {
            Keyed::Insert { before, .. } | Keyed::Move { before, .. } => before,
            _ => continue,
        };
        if let Anchor::Before(Key::Id(id)) | Anchor::After(Key::Id(id)) = a {
            out.push(id.clone());
        }
    }
    out
}

/// Items a committed ripple move put somewhere (an insertion with a shift).
fn ripple_moved(since: &[CommittedTx]) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for c in since {
        for ch in &c.diff.changes {
            if let Op::ItemMove(m) = &c.tx.ops[ch.op as usize]
                && m.mode == InsertMode::Ripple
                && ch.path.as_deref() == Some("start")
            {
                out.insert(ch.id.clone());
            }
        }
    }
    out
}

/// Items a committed move or slide relocated or re-edged without shifts.
fn relocated(since: &[CommittedTx]) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for c in since {
        for ch in &c.diff.changes {
            let op = &c.tx.ops[ch.op as usize];
            if matches!(op, Op::ItemMove(_) | Op::ItemSlide(_)) {
                out.insert(ch.id.clone());
            }
            if let Op::ItemMove(m) = op
                && m.mode == InsertMode::Overwrite
            {
                out.insert(ch.id.clone());
            }
        }
    }
    out
}

fn lens() -> impl Strategy<Value = Vec<Vec<i64>>> {
    prop::collection::vec(prop::collection::vec(10i64..40, 0..4), 4)
}

proptest! {
    #![proptest_config(prop_config(256))]
    #[test]
    fn stale_and_fresh_agree_with_moves_links_transitions(
        lens in lens(),
        linked in any::<bool>(),
        trs in any::<bool>(),
        committed in prop::collection::vec(
            (prop::collection::vec(intent(), 1..3), any::<bool>()),
            0..5,
        ),
        mine in prop::collection::vec(intent(), 1..4),
    ) {
        let p = base(&lens, linked, trs);
        let mut ids = MemIds::after(&p);
        let mut head = p.clone();
        let mut since: Vec<CommittedTx> = Vec::new();
        for (intents, undo_it) in &committed {
            let rev = since.len() as u64 + 1;
            let Some((next, c, inverse)) = commit(&head, intents, rev, &mut ids) else { continue };
            prop_assert_eq!(transitions_valid(&next), Ok(()));
            let rp = fin_core::replay::apply_committed(&head, &c.tx, &c.bindings).unwrap();
            prop_assert_eq!(&rp.project, &next, "replay state differs");
            prop_assert_eq!(&rp.diff, &c.diff, "replay diff differs");
            prop_assert_eq!(&rp.inverse, &inverse, "replay inverse differs");
            let before = head.clone();
            head = next;
            since.push(c);
            if *undo_it {
                let Some((next, u)) = undo(&head, since.last().unwrap(), inverse, rev + 1) else {
                    return Ok(());
                };
                prop_assert_eq!(&next, &before, "undo not exact: {:?}", since.last().unwrap().tx.ops);
                since.push(u);
                head = next;
            }
        }
        let mut at_base = Realiser::with_ids(&p, ids.clone());
        let keyed = at_base.keyed(&mine, "o");
        if at_base.ops.is_empty() {
            return Ok(());
        }
        let mut at_head = Realiser::with_ids(&head, ids.clone());
        let moved = ripple_moved(&since);
        let mut b: std::collections::BTreeSet<String> = p.timelines[0].tracks.iter().flat_map(|t| &t.items).map(|c| c.id.clone()).filter(|i| !moved.contains(i)).collect();
        b.extend(relocated(&since).into_iter().filter(|i| !moved.contains(i)));
        at_head.base = Some(b);
        let fresh_ok = keyed.iter().all(|k| at_head.realise(k).is_some());
        let mut t = tx(at_base.ops.clone());
        t.tx_id = "tx-ours".into();
        let stale = rebase_tx(&p, &head, &t, &since, &mut ids.clone());
        if let Ok(s) = &stale {
            assert_well_formed(&s.applied.project);
            prop_assert_eq!(transitions_valid(&s.applied.project), Ok(()));
            let rp = fin_core::replay::apply_committed(&head, &s.tx, &s.applied.bindings).unwrap();
            prop_assert_eq!(&rp.project, &s.applied.project, "replay of rebased differs");
            prop_assert_eq!(&rp.diff, &s.applied.diff, "replay diff of rebased differs");
            let u = apply_internal(&s.applied.project, &tx(s.applied.inverse.clone()),
                &mut MemIds::after(&s.applied.project)).unwrap();
            prop_assert_eq!(&u.project, &head, "rebased tx does not undo exactly: {:?}", s.tx.ops);
        }
        match (fresh_ok, stale) {
            (true, Ok(stale)) if !keyed.iter().any(|k| matches!(k, Keyed::Insert { before: Anchor::Zero, .. } | Keyed::Move { before: Anchor::Zero, .. })) && !anchor_ids(&keyed).iter().any(|i| relocated(&since).contains(i))
                && !self_relocates(&keyed) && !group_grew(&keyed, &since) && !partner_anchor(&keyed, &p)
                && !lifted_on(&keyed, &since) => {
                let mut ft = tx(at_head.ops.clone());
                ft.tx_id = "tx-ours".into();
                let fresh = apply_tx(&head, &ft, &mut ids.clone()).unwrap();
                prop_assert_eq!(&stale.applied.project, &fresh.project,
                    "stale {:?}\nfresh {:?}", stale.tx.ops, ft.ops);
            }
            (true, Err(e)) => {
                let reason = e.details["reason"].as_str().unwrap_or(&e.code).to_string();
                prop_assert!(
                    e.code == "CONFLICT" && !reason.is_empty(),
                    "stale refused ({}) where fresh applies: {:?}\nours {:?}\nfresh {:?}",
                    reason, e, at_base.ops, at_head.ops
                );
            }
            _ => {}
        }
    }
}
