//! The keyed intent model of the stale-vs-fresh properties (4C Tasks 9–10 review, widened by
//! Task 26; extends 4B's `rebase_props`): intents named by entity identity, realised at the base
//! (then rebased over committed records, some undone) and again at the head (fresh), with the
//! known limitations under which the two may differ ([`comparable`]). Shared by
//! `rebase_moves_props` (moves, slides, links, transitions) and `props_rebase_4c` (every 4C edit).
//!
//! Since 2026-11-09 positions map through the item edge they were written against (spec §4.3,
//! `rebase::anchors`), which is what this model's [`Anchor`] names. The shift-only limitations
//! this model used to leave out of the comparison or allow as unblamed refusals (an anchor at an
//! empty track's zero, a relocated anchor, self relocation, group grew or changed, partner
//! anchor and drift, lifted, lifted edges, anchor drift and lifted gap) are gone: every case is
//! compared, and a refusal must be one the spec explains ([`refusal_allowed`]). Each was checked
//! by its fixed case passing under the strict check. The one exclusion left,
//! [`index_in_changed_list`], belongs to `rebase::places`, which has its own oracle.
//! An [`Anchor::After`] (`before: None`) is an append: the track's end at the head, as the
//! engine reads a position at the end of the track's last item.
#![allow(dead_code)]

use std::collections::BTreeMap;

use crate::common::gen4c::{self, INTER, NOTO, catalog_pin, inter, sha};
use crate::common::moves::{link, move_to, slide, unlink};
use crate::common::*;
use fin_core::{
    CaptionMerge, CaptionSplit, CommittedTx, Curve, Edge, EffectAdd, EffectMove, InsertMode,
    Interp, ItemSlip, ItemSplit, ItemTrim, KeyframeRemove, KeyframeSet, Link, MemIds, Op, ParamSet,
    ParamValue, Project, Rational, Track, TrackKind, TransitionKind, TransitionRemove,
    TransitionSet, TrimMode, apply_internal, apply_tx, rebase_tx,
};
use proptest::prelude::*;

mod four;
pub use four::*;

/// t1 V1, t2 A1, t3 A2, t4 V2, and (in [`base_4c`]) t5 C1 for captions.
pub const TRACKS: [&str; 5] = ["t1", "t2", "t3", "t4", "t5"];

pub fn frames(n: i64) -> Rational {
    Rational::new(n, 30).unwrap()
}

pub fn kind_of(track: &str) -> TransitionKind {
    if track == "t1" || track == "t4" {
        TransitionKind::Dissolve
    } else {
        TransitionKind::Crossfade
    }
}

/// t1 V1, t2 A1, t3 A2, t4 V2; back-to-back clips; optionally c0_0 linked with c1_0, and a
/// transition at every cut.
pub fn base(lens: &[Vec<i64>], linked: bool, trs: bool) -> Project {
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
pub enum Intent {
    Insert {
        track: usize,
        before: Option<usize>,
        len: i64,
    },
    /// `mode` 0 ripple, 1 roll, 2 overwrite.
    Trim {
        track: usize,
        clip: usize,
        start: bool,
        frames: i64,
        mode: u8,
    },
    Delete {
        track: usize,
        clip: usize,
    },
    Split {
        track: usize,
        clip: usize,
        eighths: i64,
    },
    Slip {
        track: usize,
        clip: usize,
        frames: i64,
    },
    Opacity {
        track: usize,
        clip: usize,
        v: i64,
    },
    /// `fx`: the first effect's param, else opacity; `at` frames into the item.
    KeySet {
        track: usize,
        clip: usize,
        fx: bool,
        at: i64,
        v: i64,
    },
    KeyRemove {
        track: usize,
        clip: usize,
        j: usize,
    },
    EffectAdd {
        track: usize,
        clip: usize,
        blur: bool,
        at: Option<u32>,
    },
    EffectMove {
        track: usize,
        clip: usize,
        to: u32,
    },
    CapSplit {
        k: usize,
        word: u32,
    },
    CapMerge {
        k: usize,
    },
    /// `which` 0 Inter (`a`), 1 Inter (`b`), 2 Noto.
    Pin {
        which: u8,
        fallback: Option<u32>,
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

pub fn intent() -> impl Strategy<Value = Intent> {
    let t = 0usize..4;
    let c = 0usize..6;
    prop_oneof![
        1 => (t.clone(), prop::option::of(c.clone()), 1i64..40)
            .prop_map(|(track, before, len)| Intent::Insert { track, before, len }),
        1 => (t.clone(), c.clone(), any::<bool>(), -8i64..8)
            .prop_filter("moves", |x| x.3 != 0)
            .prop_map(|(track, clip, start, frames)| Intent::Trim { track, clip, start, frames, mode: 0 }),
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
pub enum Key {
    Id(String),
    Own(String),
}

#[derive(Debug, Clone)]
pub enum Anchor {
    Before(Key),
    After(Key),
    Zero,
}

#[derive(Debug, Clone)]
pub enum Keyed {
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
        mode: TrimMode,
    },
    Delete {
        clip: Key,
    },
    /// `at` is item-relative where written; `src` the item's `sourceIn` there (B3: the same
    /// content at the head is at `at − (sourceIn − src − committed slips)`).
    Split {
        name: String,
        clip: Key,
        at: Rational,
        src: Rational,
    },
    Slip {
        clip: Key,
        frames: i64,
    },
    Opacity {
        clip: Key,
        v: i64,
    },
    /// `fx`: the effect whose radius it keys, else opacity; `at`/`src` as for a split.
    KeySet {
        clip: Key,
        fx: Option<Key>,
        at: Rational,
        src: Rational,
        v: i64,
    },
    KeyRemove {
        clip: Key,
        fx: Option<Key>,
        param: String,
        at: Rational,
        src: Rational,
    },
    EffectAdd {
        name: String,
        clip: Key,
        blur: bool,
        at: Option<u32>,
    },
    EffectMove {
        clip: Key,
        fx: Key,
        to: u32,
    },
    CapSplit {
        name: String,
        cap: Key,
        word: u32,
    },
    CapMerge {
        cap: Key,
        with: Key,
    },
    Pin {
        which: u8,
        fallback: Option<u32>,
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

pub struct Realiser {
    state: Project,
    ids: MemIds,
    bound: BTreeMap<String, String>,
    ops: Vec<Op>,
    /// At the head: the committed slips per item (they move the source, not the content).
    pub slips: BTreeMap<String, Rational>,
}

pub fn reference(key: &Key) -> String {
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
            slips: BTreeMap::new(),
        }
    }

    /// The item `key` names, and its `sourceIn` (clips only).
    fn clip_src(&self, key: &Key) -> Option<(fin_core::Item, Rational)> {
        let (t, i) = self.item(key)?;
        let it = self.state.timelines[0].tracks[t].items[i].clone();
        let src = it.source_in()?;
        it.asset()?;
        Some((it, src))
    }

    /// Where content at item-relative `at` (written where the item's `sourceIn` was `src`) is
    /// now, on the item `key` names.
    /// An item our own earlier op made (an insert, a split's right part) holds the same content
    /// at the head: nothing committed touched it.
    fn content_at(&self, key: &Key, at: Rational, src: Rational) -> Option<Rational> {
        let (it, now) = self.clip_src(key)?;
        if matches!(key, Key::Own(_)) {
            return Some(at);
        }
        let slipped = self.slips.get(&it.id).copied().unwrap_or_default();
        let moved = now.checked_sub(src).ok()?.checked_sub(slipped).ok()?;
        at.checked_sub(moved).ok()
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
                    // `before: None` is an append: the track's end, whatever the head
                    // appended or moved there since (spec §4.3, `rebase::anchors`).
                    _ => tracks[t]
                        .items
                        .iter()
                        .map(|n| (n.start + n.duration).unwrap())
                        .max()
                        .unwrap(),
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
                mode,
            } => {
                self.item(clip)?;
                ItemTrim {
                    item: reference(clip),
                    edge: *edge,
                    delta: frames(*n),
                    mode: *mode,
                    alone: false,
                }
                .into()
            }
            Keyed::Split {
                name,
                clip,
                at,
                src,
            } => ItemSplit {
                item: reference(clip),
                offset: self.content_at(clip, *at, *src)?,
                id: name.clone(),
            }
            .into(),
            Keyed::Slip { clip, frames: n } => {
                self.item(clip)?;
                ItemSlip {
                    item: reference(clip),
                    delta: frames(*n),
                }
                .into()
            }
            Keyed::Opacity { clip, v } => {
                self.item(clip)?;
                ParamSet {
                    item: reference(clip),
                    path: "opacity".into(),
                    value: Some(ParamValue::Number(*v as f64 / 10.0)),
                }
                .into()
            }
            Keyed::KeySet {
                clip,
                fx,
                at,
                src,
                v,
            } => KeyframeSet {
                item: reference(clip),
                path: match fx {
                    Some(fx) => format!("effects.{}.radius", reference(fx)),
                    None => "opacity".into(),
                },
                at: self.content_at(clip, *at, *src)?,
                value: ParamValue::Number(*v as f64 / 10.0),
                interp: Interp::Linear,
            }
            .into(),
            Keyed::KeyRemove {
                clip,
                fx,
                param,
                at,
                src,
            } => KeyframeRemove {
                item: reference(clip),
                path: match fx {
                    Some(fx) => format!("effects.{}.{param}", reference(fx)),
                    None => param.clone(),
                },
                at: self.content_at(clip, *at, *src)?,
            }
            .into(),
            Keyed::EffectAdd {
                name,
                clip,
                blur,
                at,
            } => {
                self.item(clip)?;
                EffectAdd {
                    item: reference(clip),
                    id: name.clone(),
                    kind: if *blur { "blur" } else { "colorAdjust" }.into(),
                    at: *at,
                    params: BTreeMap::new(),
                }
                .into()
            }
            Keyed::EffectMove { clip, fx, to } => {
                self.item(clip)?;
                EffectMove {
                    item: reference(clip),
                    effect: reference(fx),
                    to: *to,
                }
                .into()
            }
            Keyed::CapSplit { name, cap, word } => {
                self.item(cap)?;
                CaptionSplit {
                    item: reference(cap),
                    word: *word,
                    id: name.clone(),
                }
                .into()
            }
            Keyed::CapMerge { cap, with } => {
                self.item(cap)?;
                self.item(with)?;
                CaptionMerge {
                    item: reference(cap),
                    with: reference(with),
                }
                .into()
            }
            Keyed::Pin { which, fallback } => match which {
                0 => gen4c::pin(INTER, inter(&sha('a')), *fallback),
                1 => gen4c::pin(INTER, inter(&sha('b')), *fallback),
                _ => gen4c::pin(NOTO, catalog_pin("Noto Sans", 400, &sha('d')), *fallback),
            },
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

    /// The `k`th clip (mod their count) for which `want` holds.
    fn rich(
        &self,
        k: usize,
        want: impl Fn(&fin_core::Item) -> bool,
    ) -> Option<(Key, fin_core::Item, Rational)> {
        let all: Vec<&fin_core::Item> = self.state.timelines[0]
            .tracks
            .iter()
            .flat_map(|t| &t.items)
            .filter(|i| i.source_in().is_some() && want(i))
            .collect();
        let it = (*all.get(k % all.len().max(1))?).clone();
        Some((self.key_of(&it.id), it.clone(), it.source_in()?))
    }

    /// The keyed form of a 4C intent (Task 26) at this state.
    fn keyed_4c(&self, intent: &Intent, name: String) -> Option<Keyed> {
        let clip_of = |track: usize, clip: usize| -> Option<(Key, fin_core::Item, Rational)> {
            let key = self.pick(track, clip)?;
            let (it, src) = self.clip_src(&key)?;
            Some((key, it, src))
        };
        let whole = |r: Rational| r.num() * 30 / r.den();
        Some(match *intent {
            Intent::Split {
                track,
                clip,
                eighths,
            } => {
                let (clip, it, src) = clip_of(track, clip)?;
                Keyed::Split {
                    name,
                    clip,
                    at: frames(whole(it.duration) * eighths / 8),
                    src,
                }
            }
            Intent::Slip {
                track,
                clip,
                frames,
            } => Keyed::Slip {
                clip: self.pick(track, clip)?,
                frames,
            },
            Intent::Opacity { track, clip, v } => Keyed::Opacity {
                clip: self.pick(track, clip)?,
                v,
            },
            Intent::KeySet {
                track,
                clip,
                fx,
                at,
                v,
            } => {
                let (clip, it, src) = clip_of(track, clip)?;
                let fx = match fx {
                    true => Some(self.key_of(&it.common.effects.first()?.id)),
                    false => None,
                };
                Keyed::KeySet {
                    clip,
                    fx,
                    at: frames(at % whole(it.duration).max(1)),
                    src,
                    v,
                }
            }
            Intent::KeyRemove { track, clip, j } => {
                let (clip, it, src) =
                    self.rich(track + clip, |i| !i.common.keyframes.is_empty())?;
                let all: Vec<(&String, Rational)> = it
                    .common
                    .keyframes
                    .iter()
                    .flat_map(|(p, ks)| ks.iter().map(move |x| (p, x.at)))
                    .collect();
                let (path, at) = *all.get(j % all.len().max(1))?;
                let (fx, param) = match fin_core::params::effect_path(path) {
                    Some((fx, rest)) => (Some(self.key_of(fx)), rest.to_string()),
                    None => (None, path.clone()),
                };
                Keyed::KeyRemove {
                    clip,
                    fx,
                    param,
                    at,
                    src,
                }
            }
            Intent::EffectAdd {
                track,
                clip,
                blur,
                at,
            } => Keyed::EffectAdd {
                name,
                clip: clip_of(track, clip)?.0,
                blur,
                at,
            },
            Intent::EffectMove { track, clip, to } => {
                let (clip, it, _) = self.rich(track + clip, |i| i.common.effects.len() > 1)?;
                Keyed::EffectMove {
                    clip,
                    fx: self.key_of(&it.common.effects.first()?.id),
                    to,
                }
            }
            Intent::CapSplit { k, word } => Keyed::CapSplit {
                name,
                cap: self.pick(4, k)?,
                word,
            },
            Intent::CapMerge { k } => {
                let caps = &self.state.timelines[0].tracks.get(4)?.items;
                let i = k % caps.len().max(1);
                Keyed::CapMerge {
                    cap: self.key_of(&caps.get(i)?.id),
                    with: self.key_of(&caps.get(i + 1)?.id),
                }
            }
            Intent::Pin { which, fallback } => Keyed::Pin { which, fallback },
            _ => return None,
        })
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
                    mode,
                } => {
                    let Some(clip) = self.pick(track, clip) else {
                        continue;
                    };
                    let edge = if start { Edge::Start } else { Edge::End };
                    let mode =
                        [TrimMode::Ripple, TrimMode::Roll, TrimMode::Overwrite][mode as usize];
                    Keyed::Trim {
                        clip,
                        edge,
                        frames,
                        mode,
                    }
                }
                ref i @ (Intent::Split { .. }
                | Intent::Slip { .. }
                | Intent::Opacity { .. }
                | Intent::KeySet { .. }
                | Intent::KeyRemove { .. }
                | Intent::EffectAdd { .. }
                | Intent::EffectMove { .. }
                | Intent::CapSplit { .. }
                | Intent::CapMerge { .. }
                | Intent::Pin { .. }) => {
                    let Some(k) = self.keyed_4c(i, name) else {
                        continue;
                    };
                    k
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

pub fn commit(
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

pub fn undo(
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
pub fn transitions_valid(p: &Project) -> Result<(), String> {
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

pub fn lens() -> impl Strategy<Value = Vec<Vec<i64>>> {
    prop::collection::vec(prop::collection::vec(10i64..40, 0..4), 4)
}

/// The stale-vs-fresh check of one case: committed intents (each sometimes undone) at `p`, then
/// `mine` realised at `p` and rebased, against `mine` realised at the head.
pub fn check(
    p: &Project,
    committed: &[(Vec<Intent>, bool)],
    mine: &[Intent],
) -> Result<(), TestCaseError> {
    let mut ids = MemIds::after(p);
    let mut head = p.clone();
    let mut since: Vec<CommittedTx> = Vec::new();
    for (intents, undo_it) in committed {
        let rev = since.len() as u64 + 1;
        let Some((next, c, inverse)) = commit(&head, intents, rev, &mut ids) else {
            continue;
        };
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
            prop_assert_eq!(
                &next,
                &before,
                "undo not exact: {:?}",
                since.last().unwrap().tx.ops
            );
            since.push(u);
            head = next;
        }
    }
    let mut at_base = Realiser::with_ids(p, ids.clone());
    let keyed = at_base.keyed(mine, "o");
    if at_base.ops.is_empty() {
        return Ok(());
    }
    let mut at_head = Realiser::with_ids(&head, ids.clone());
    at_head.slips = slips(&since);
    let fresh_ok = keyed.iter().all(|k| at_head.realise(k).is_some());
    let mut t = tx(at_base.ops.clone());
    t.tx_id = "tx-ours".into();
    let stale = rebase_tx(p, &head, &t, &since, &mut ids.clone());
    if let Ok(s) = &stale {
        assert_well_formed(&s.applied.project);
        prop_assert_eq!(transitions_valid(&s.applied.project), Ok(()));
        let rp = fin_core::replay::apply_committed(&head, &s.tx, &s.applied.bindings).unwrap();
        prop_assert_eq!(&rp.project, &s.applied.project, "replay of rebased differs");
        prop_assert_eq!(&rp.diff, &s.applied.diff, "replay diff of rebased differs");
        let u = apply_internal(
            &s.applied.project,
            &tx(s.applied.inverse.clone()),
            &mut MemIds::after(&s.applied.project),
        )
        .unwrap();
        prop_assert_eq!(
            &u.project,
            &head,
            "rebased tx does not undo exactly: {:?}",
            s.tx.ops
        );
    }
    match (fresh_ok, stale) {
        (true, Ok(stale)) if comparable(&keyed, &since, p) => {
            let mut ft = tx(at_head.ops.clone());
            ft.tx_id = "tx-ours".into();
            let fresh = apply_tx(&head, &ft, &mut ids.clone()).unwrap();
            prop_assert_eq!(
                &stale.applied.project,
                &fresh.project,
                "stale {:?}\nfresh {:?}",
                stale.tx.ops,
                ft.ops
            );
        }
        (true, Err(e)) => {
            let why = refusal_allowed(&e, &since);
            prop_assert!(
                why.is_ok(),
                "stale refused ({:?}) where fresh applies\nours {:?}\nfresh {:?}",
                why,
                at_base.ops,
                at_head.ops
            );
        }
        _ => {}
    }
    Ok(())
}

/// Whether the stale and fresh states must be equal: unless one of ours names an index into a
/// list a committed record changed ([`index_in_changed_list`]), always. Positions map through
/// the item edge they were written against (spec §4.3, `rebase::anchors`, 2026-11-09), so the
/// shift-only limitations this used to leave out (an anchor at an empty track's zero, a
/// relocated anchor, self relocation, group grew or changed, partner anchor, lifted, lifted
/// edges) are compared like every other case.
pub fn comparable(keyed: &[Keyed], since: &[CommittedTx], _p: &Project) -> bool {
    !index_in_changed_list(keyed, since)
}

/// Whether one of ours names an index into an ordered list (an item's effects, the font
/// fallback order) that a committed record changed since. The fresh realisation names the same
/// absolute index at the head; the stale edit is carried by its neighbours (spec §4.3,
/// `rebase::places`), which is only the same place while the list kept its base order. That
/// rule has its own independent oracle (`props_places`); here the stale edit is still
/// replayed, undone exactly, and refused only for an allowed reason.
pub fn index_in_changed_list(keyed: &[Keyed], since: &[CommittedTx]) -> bool {
    let effects = keyed.iter().any(|k| {
        matches!(
            k,
            Keyed::EffectAdd { at: Some(_), .. } | Keyed::EffectMove { .. }
        )
    });
    let fallback = keyed.iter().any(|k| {
        matches!(
            k,
            Keyed::Pin {
                fallback: Some(_),
                ..
            }
        )
    });
    since.iter().flat_map(|c| &c.tx.ops).any(|op| match op {
        Op::EffectAdd(_) | Op::EffectMove(_) | Op::EffectRemove(_) | Op::EffectRestore(_) => {
            effects
        }
        Op::FontPin(_) | Op::FontUnpin(_) => fallback,
        _ => false,
    })
}
