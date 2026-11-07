//! Generators for the 4C property suite (plan 4C Task 26): a base project holding every kind
//! of 4C entity, and abstract ops ([`G`]) realised against the current state ([`World`]) into
//! concrete client or internal ops of every kind. An abstract op that does not apply where it
//! is realised is skipped.
#![allow(dead_code)]

use std::collections::BTreeMap;

use fin_core::{
    ApplyMode, AssetAdd, AssetKind, AssetMove, BinAdd, BinMove, BinRemove, CaptionBlock,
    CaptionMerge, CaptionSetStyle, CaptionSetText, CaptionSplit, Curve, DeleteMode, Edge,
    EffectAdd, EffectMove, EffectRemove, FontFace, FontFile, FontFormat, FontPin, FontPinOp,
    FontRepin, FontSource, FontUnpin, InsertMode, Interp, Item, ItemBody, ItemDelete, ItemInsert,
    ItemLink, ItemMove, ItemRemove, ItemRestore, ItemSlide, ItemSlip, ItemSplit, ItemTrim,
    ItemUnlink, KeyframeRemove, KeyframeSet, LutInfo, MarkerAdd, MarkerRemove, MarkerSet, MemIds,
    Op, ParamSet, ParamValue, Project, Rational, Resolution, StyleAdd, StyleRemove, StyleSet,
    TimelineAdd, TimelineRemove, TimelineSet, Track, TrackAdd, TrackKind, TrackMove, TrackRemove,
    TrackSet, TransitionKind, TransitionRemove, TransitionSet, TrimMode, Word, apply_tx_mode,
};
use proptest::prelude::*;

use super::{apply, audio_asset, av_asset, project, secs, tx};

/// `t1` V1, `t2` A1, `t3` C1 (captions), `t4` V2.
pub const TRACKS: [&str; 4] = ["t1", "t2", "t3", "t4"];
pub const INTER: &str = "Inter|700|normal";
pub const NOTO: &str = "Noto Sans|400|normal";
pub const TEST_SANS: &str = "Test Sans|400|normal";

pub fn frames(n: i64) -> Rational {
    Rational::new(n, 30).unwrap()
}

pub fn sha(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

pub fn catalog_pin(family: &str, weight: u16, sha256: &str) -> FontPin {
    FontPin {
        family: family.into(),
        weight,
        italic: false,
        face: 0,
        sha256: sha256.into(),
        size: 1000,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Catalog {
            entry: "e".into(),
            style: "s".into(),
            catalog: "2026.11.1".into(),
        },
    }
}

pub fn inter(sha256: &str) -> FontPin {
    catalog_pin("Inter", 700, sha256)
}

pub fn pin(key: &str, p: FontPin, fallback: Option<u32>) -> Op {
    FontPinOp {
        key: key.into(),
        pin: p,
        fallback,
    }
    .into()
}

/// A font asset (`Test Sans`, whole-file SHA-256 `c…`).
pub fn font_asset(id: &str) -> Op {
    let mut a = AssetAdd::media(id, "/fonts/test.ttf", secs(1), None, None, "fp");
    a.kind = AssetKind::Font;
    a.duration = None;
    a.font = Some(FontFile {
        sha256: sha('c'),
        size: 1234,
        format: FontFormat::Ttf,
        faces: vec![FontFace {
            index: 0,
            family: "Test Sans".into(),
            weight: 400,
            italic: false,
            scripts: vec!["Latn".into()],
        }],
    });
    a.into()
}

pub fn asset_pin(asset: &str) -> FontPin {
    FontPin {
        family: "Test Sans".into(),
        weight: 400,
        italic: false,
        face: 0,
        sha256: sha('c'),
        size: 1234,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Asset {
            asset: asset.into(),
        },
    }
}

pub fn lut_asset(id: &str) -> Op {
    let mut a = AssetAdd::media(id, "/m/look.cube", secs(1), None, None, "fp");
    a.kind = AssetKind::Lut;
    a.duration = None;
    a.lut = Some(LutInfo { size: 33 });
    a.into()
}

/// Words of `n` frames each from item-relative 0.
pub fn words(texts: &[&str], n: i64) -> Vec<Word> {
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| Word {
            text: (*t).into(),
            start: frames(i as i64 * n),
            end: frames((i as i64 + 1) * n),
        })
        .collect()
}

pub fn caption(id: &str, track: &str, at: Rational, texts: &[&str], style: &str) -> Op {
    ItemInsert {
        id: id.into(),
        track: track.into(),
        at,
        duration: frames(15 * texts.len() as i64),
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words: words(texts, 15),
                style: style.into(),
                overrides: BTreeMap::new(),
                source: None,
            },
        },
    }
    .into()
}

fn clip_body(asset: &str, source_in: Rational) -> ItemBody {
    ItemBody::Clip {
        asset: asset.into(),
        source_in,
        speed: secs(1),
    }
}

pub fn tr_kind(track: &Track) -> TransitionKind {
    if track.kind == TrackKind::Audio {
        TransitionKind::Crossfade
    } else {
        TransitionKind::Dissolve
    }
}

/// What the base project holds besides its clips.
#[derive(Debug, Clone)]
pub struct Shape {
    /// Clip lengths in frames on `t1`, `t2` and `t4`.
    pub lens: Vec<Vec<i64>>,
    pub linked: bool,
    pub transitions: bool,
    pub captions: usize,
    pub effects: bool,
    pub pinned: bool,
}

pub fn shape() -> impl Strategy<Value = Shape> {
    (
        prop::collection::vec(prop::collection::vec(12i64..60, 0..4), 3),
        any::<bool>(),
        any::<bool>(),
        0usize..4,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(lens, linked, transitions, captions, effects, pinned)| Shape {
                lens,
                linked,
                transitions,
                captions,
                effects,
                pinned,
            },
        )
}

/// Assets `a1` (A/V, 100 s), `a2` (audio, 40 s), `a3` (a LUT) and `a4` (a font); style
/// `st1`, bin `bin1` and marker `m1`; back-to-back clips 10 s into `a1` on `t1`, `t2`, `t4`; captions of two or three words
/// on `t3`; optionally the first clips of `t1` and `t2` linked, a transition at every cut, a
/// blur with radius keyframes and a colour adjustment, and opacity keyframes on the first clip of `t1`, and pins
/// (`Inter` in the fallback order, the font asset's face).
pub fn base4c(s: &Shape) -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    p.timelines[0]
        .tracks
        .push(Track::new("t4", TrackKind::Video, "V2"));
    let mut ops = vec![
        av_asset("$a", secs(100)),
        audio_asset("$b", secs(40)),
        lut_asset("$l"),
        font_asset("$f"),
        StyleAdd {
            id: "$st".into(),
            name: "Big".into(),
            from: None,
            set: BTreeMap::from([("text.size".into(), ParamValue::Number(80.0))]),
        }
        .into(),
        BinAdd {
            id: "$bin".into(),
            name: "Media".into(),
            parent: None,
        }
        .into(),
        MarkerAdd {
            id: "$m".into(),
            timeline: "tl1".into(),
            at: frames(30),
            duration: None,
            label: "start".into(),
            color: None,
        }
        .into(),
    ];
    for (t, lens) in s.lens.iter().enumerate() {
        let track = ["t1", "t2", "t4"][t];
        let mut at = 0;
        for (i, n) in lens.iter().enumerate() {
            let id = format!("$c{t}_{i}");
            ops.push(
                ItemInsert {
                    id: id.clone(),
                    track: track.into(),
                    at: frames(at),
                    duration: frames(*n),
                    mode: InsertMode::Ripple,
                    body: clip_body("$a", secs(10)),
                }
                .into(),
            );
            if s.transitions && i > 0 {
                ops.push(
                    TransitionSet {
                        id: format!("$tr{t}_{i}"),
                        track: track.into(),
                        between: [format!("$c{t}_{}", i - 1), id],
                        kind: if t == 1 {
                            TransitionKind::Crossfade
                        } else {
                            TransitionKind::Dissolve
                        },
                        duration: frames(6),
                        curve: Curve::Linear,
                    }
                    .into(),
                );
            }
            at += n;
        }
    }
    let texts: [&[&str]; 3] = [
        &["one", "two"],
        &["three", "four", "five"],
        &["six", "seven"],
    ];
    for (i, words) in texts.iter().take(s.captions).enumerate() {
        let style = if i == 1 { "$st" } else { "preset:bold" };
        ops.push(caption(
            &format!("$cap{i}"),
            "t3",
            frames(i as i64 * 45),
            words,
            style,
        ));
    }
    if s.linked && !s.lens[0].is_empty() && !s.lens[1].is_empty() {
        ops.push(
            ItemLink {
                items: vec!["$c0_0".into(), "$c1_0".into()],
                id: "$lk".into(),
            }
            .into(),
        );
    }
    if s.effects && !s.lens[0].is_empty() {
        ops.push(
            EffectAdd {
                item: "$c0_0".into(),
                id: "$fx".into(),
                kind: "blur".into(),
                at: None,
                params: BTreeMap::new(),
            }
            .into(),
        );
        ops.push(
            EffectAdd {
                item: "$c0_0".into(),
                id: "$fx2".into(),
                kind: "colorAdjust".into(),
                at: None,
                params: BTreeMap::new(),
            }
            .into(),
        );
        for (at, v) in [(0, 0.0), (6, 10.0)] {
            ops.push(key_op("$c0_0", "effects.$fx.radius", frames(at), v));
            ops.push(key_op("$c0_0", "opacity", frames(at + 3), 1.0 - v / 20.0));
        }
    }
    if s.pinned {
        ops.push(pin(INTER, inter(&sha('a')), Some(0)));
        ops.push(pin(TEST_SANS, asset_pin("$f"), None));
    }
    apply(&p, ops).project
}

pub fn key_op(item: &str, path: &str, at: Rational, v: f64) -> Op {
    KeyframeSet {
        item: item.into(),
        path: path.into(),
        at,
        value: ParamValue::Number(v),
        interp: Interp::Linear,
    }
    .into()
}

// ---------------------------------------------------------------------------------------------
// Abstract ops.

/// An abstract op: entities picked by index (mod their count) in the state it is realised in.
#[derive(Debug, Clone)]
pub enum G {
    /// `kind` 0 a clip, 1 an adjustment, 2 a caption, 3 a nested item; at item `k`'s start
    /// plus `off` frames (or the track end).
    Insert {
        t: usize,
        k: Option<usize>,
        off: i64,
        len: i64,
        overwrite: bool,
        kind: u8,
    },
    /// `mode` 0 ripple, 1 roll, 2 overwrite, 3 strict (internal).
    Trim {
        t: usize,
        k: usize,
        start: bool,
        n: i64,
        mode: u8,
    },
    Delete {
        t: usize,
        k: usize,
        lift: bool,
    },
    /// Internal: `item.remove`.
    Remove {
        t: usize,
        k: usize,
        lift: bool,
    },
    /// Internal: an item removed earlier, under its id or a fresh one.
    Restore {
        k: usize,
        fresh: bool,
        lift: bool,
    },
    Slip {
        t: usize,
        k: usize,
        n: i64,
    },
    Split {
        t: usize,
        k: usize,
        eighths: i64,
    },
    Move {
        t: usize,
        k: usize,
        to: usize,
        at: Option<usize>,
        off: i64,
        overwrite: bool,
    },
    /// A ripple move of a linked item (`k`th of them), so every member moves (Decision 23:
    /// the inverse's parts, one per member, in reverse order).
    MoveLinked {
        k: usize,
        to: usize,
        at: Option<usize>,
    },
    Slide {
        t: usize,
        k: usize,
        n: i64,
    },
    Link {
        a: (usize, usize),
        b: (usize, usize),
    },
    Unlink {
        t: usize,
        k: usize,
    },
    TrSet {
        t: usize,
        k: usize,
        n: i64,
    },
    TrRemove {
        t: usize,
        k: usize,
    },
    /// `which` picks a registry path and value; 0 resets opacity.
    Param {
        t: usize,
        k: usize,
        which: u8,
        v: i64,
    },
    /// `kind` 0 blur, 1 colorAdjust, 2 lut.
    EffectAdd {
        t: usize,
        k: usize,
        kind: u8,
        at: Option<u32>,
    },
    EffectRemove {
        t: usize,
        k: usize,
        e: usize,
    },
    EffectMove {
        t: usize,
        k: usize,
        e: usize,
        to: u32,
    },
    /// `which` 0 opacity, 1 the first effect's param, 2 rotation, 3 gain; `at` frames into
    /// the item.
    KeySet {
        t: usize,
        k: usize,
        which: u8,
        at: i64,
        v: i64,
        interp: u8,
    },
    KeyRemove {
        t: usize,
        k: usize,
        j: usize,
    },
    /// `what` 0 setText, 1 split, 2 merge with the next, 3 setStyle.
    Caption {
        k: usize,
        what: u8,
        x: usize,
    },
    StyleAdd {
        from: u8,
        v: i64,
    },
    StyleSet {
        k: usize,
        which: u8,
        v: i64,
    },
    StyleRemove {
        k: usize,
    },
    /// `which` 0 Inter (`a`), 1 Inter (`b`: another file), 2 Noto, 3 the font asset's face.
    FontPin {
        which: u8,
        fallback: Option<u32>,
    },
    FontUnpin {
        k: usize,
    },
    FontRepin {
        k: usize,
        c: u8,
    },
    /// `what` 0 add, 1 remove, 2 move, 3 set.
    Track {
        what: u8,
        k: usize,
        x: usize,
    },
    /// `what` 0 add, 1 set, 2 remove.
    Timeline {
        what: u8,
        k: usize,
    },
    /// `what` 0 add, 1 set, 2 remove.
    Marker {
        what: u8,
        k: usize,
        at: i64,
    },
    /// `what` 0 add, 1 move, 2 remove.
    Bin {
        what: u8,
        k: usize,
        x: usize,
    },
    /// `what` 0 a media asset, 1 a LUT, 2 a font, 3 remove, 4 move to a bin.
    Asset {
        what: u8,
        k: usize,
        x: usize,
    },
}

fn tk() -> impl Strategy<Value = (usize, usize)> {
    (0usize..4, 0usize..6)
}

/// Structural, history-relevant edits (what props 3 and 4 commit).
pub fn g_edit() -> impl Strategy<Value = G> {
    prop_oneof![
        3 => (tk(), prop::option::of(0usize..6), -6i64..20, 6i64..45, any::<bool>(), 0u8..4)
            .prop_map(|((t, _), k, off, len, overwrite, kind)| G::Insert { t, k, off, len, overwrite, kind }),
        4 => (tk(), any::<bool>(), prop_oneof![-12i64..=-1, 1i64..=12], 0u8..3)
            .prop_map(|((t, k), start, n, mode)| G::Trim { t, k, start, n, mode }),
        2 => (tk(), any::<bool>()).prop_map(|((t, k), lift)| G::Delete { t, k, lift }),
        2 => (tk(), prop_oneof![-30i64..=-1, 1i64..=30]).prop_map(|((t, k), n)| G::Slip { t, k, n }),
        2 => (tk(), 1i64..8).prop_map(|((t, k), eighths)| G::Split { t, k, eighths }),
        3 => (tk(), 0usize..4, prop::option::of(0usize..6), -6i64..6, any::<bool>())
            .prop_map(|((t, k), to, at, off, overwrite)| G::Move { t, k, to, at, off, overwrite }),
        2 => (tk(), prop_oneof![-8i64..=-1, 1i64..=8]).prop_map(|((t, k), n)| G::Slide { t, k, n }),
        1 => (tk(), tk()).prop_map(|(a, b)| G::Link { a, b }),
        1 => tk().prop_map(|(t, k)| G::Unlink { t, k }),
        2 => (tk(), 2i64..12).prop_map(|((t, k), n)| G::TrSet { t, k, n }),
        1 => tk().prop_map(|(t, k)| G::TrRemove { t, k }),
        2 => (tk(), 0u8..7, -10i64..10).prop_map(|((t, k), which, v)| G::Param { t, k, which, v }),
        2 => (tk(), 0u8..3, prop::option::of(0u32..3))
            .prop_map(|((t, k), kind, at)| G::EffectAdd { t, k, kind, at }),
        1 => (tk(), 0usize..3).prop_map(|((t, k), e)| G::EffectRemove { t, k, e }),
        1 => (tk(), 0usize..3, 0u32..3).prop_map(|((t, k), e, to)| G::EffectMove { t, k, e, to }),
        3 => (tk(), 0u8..4, 0i64..40, 0i64..20, 0u8..3)
            .prop_map(|((t, k), which, at, v, interp)| G::KeySet { t, k, which, at, v, interp }),
        1 => (tk(), 0usize..4).prop_map(|((t, k), j)| G::KeyRemove { t, k, j }),
        3 => (0usize..4, 0u8..4, 0usize..4).prop_map(|(k, what, x)| G::Caption { k, what, x }),
        1 => (0u8..3, 20i64..90).prop_map(|(from, v)| G::StyleAdd { from, v }),
        2 => (0usize..3, 0u8..4, 20i64..90).prop_map(|(k, which, v)| G::StyleSet { k, which, v }),
        2 => (0u8..4, prop::option::of(0u32..2))
            .prop_map(|(which, fallback)| G::FontPin { which, fallback }),
    ]
}

/// Every abstract op, the internal ones and the library edits included (props 1 and 6).
pub fn g_any() -> impl Strategy<Value = G> {
    prop_oneof![
        12 => g_edit(),
        1 => (0usize..4, 0usize..4, prop::option::of(0usize..6))
            .prop_map(|(k, to, at)| G::MoveLinked { k, to, at }),
        1 => (tk(), 1i64..12, any::<bool>()).prop_map(|((t, k), n, start)| G::Trim {
            t, k, start, n: if start { n } else { -n }, mode: 3
        }),
        1 => (tk(), any::<bool>()).prop_map(|((t, k), lift)| G::Remove { t, k, lift }),
        1 => (0usize..4, any::<bool>(), any::<bool>())
            .prop_map(|(k, fresh, lift)| G::Restore { k, fresh, lift }),
        1 => (0usize..3).prop_map(|k| G::StyleRemove { k }),
        1 => (0usize..3).prop_map(|k| G::FontUnpin { k }),
        1 => (0usize..3, 0u8..3).prop_map(|(k, c)| G::FontRepin { k, c }),
        2 => (0u8..4, 0usize..6, 0usize..6).prop_map(|(what, k, x)| G::Track { what, k, x }),
        1 => (0u8..3, 0usize..3).prop_map(|(what, k)| G::Timeline { what, k }),
        1 => (0u8..3, 0usize..3, 0i64..200).prop_map(|(what, k, at)| G::Marker { what, k, at }),
        1 => (0u8..3, 0usize..3, 0usize..3).prop_map(|(what, k, x)| G::Bin { what, k, x }),
        1 => (0u8..5, 0usize..6, 0usize..3).prop_map(|(what, k, x)| G::Asset { what, k, x }),
    ]
}

// ---------------------------------------------------------------------------------------------
// Realisation.

/// The state ops are realised in, the ops so far (placeholders kept: a later op names an entity
/// an earlier one created by its placeholder) and the items taken out (for internal restores).
pub struct World {
    pub state: Project,
    pub ids: MemIds,
    pub bound: BTreeMap<String, String>,
    pub ops: Vec<Op>,
    pub graveyard: Vec<(Item, String)>,
    pub n: usize,
    /// Whether internal ops may be realised.
    pub internal: bool,
}

fn pick<T>(xs: &[T], k: usize) -> Option<&T> {
    xs.get(k % xs.len().max(1))
}

impl World {
    pub fn new(state: &Project, ids: MemIds, internal: bool) -> World {
        World {
            state: state.clone(),
            ids,
            bound: BTreeMap::new(),
            ops: Vec::new(),
            graveyard: Vec::new(),
            n: 0,
            internal,
        }
    }

    fn name(&mut self, kind: &str) -> String {
        self.n += 1;
        format!("${kind}{}", self.n)
    }

    /// An id as this tx names it: the placeholder that bound it, or the id itself.
    pub fn key(&self, id: &str) -> String {
        match self.bound.iter().find(|(_, v)| *v == id) {
            Some((p, _)) => p.clone(),
            None => id.to_string(),
        }
    }

    fn track(&self, t: usize) -> Option<&Track> {
        let id = TRACKS[t % 4];
        self.state.timelines[0].tracks.iter().find(|x| x.id == id)
    }

    fn item(&self, t: usize, k: usize) -> Option<Item> {
        pick(&self.track(t)?.items, k).cloned()
    }

    fn all_items(&self) -> Vec<&Item> {
        self.state
            .timelines
            .iter()
            .flat_map(|tl| &tl.tracks)
            .flat_map(|t| &t.items)
            .collect()
    }

    /// An item with effects (`t + k` picks one of them).
    fn with_effects(&self, t: usize, k: usize) -> Option<Item> {
        let all: Vec<&Item> = self
            .all_items()
            .into_iter()
            .filter(|i| !i.common.effects.is_empty())
            .collect();
        pick(&all, t + k).map(|i| (*i).clone())
    }

    fn end_of(&self, t: usize) -> Option<Rational> {
        let items = &self.track(t)?.items;
        Some(
            items
                .last()
                .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap()),
        )
    }

    /// `op` with the ids it names resolved through the placeholders bound so far.
    pub fn resolved(&self, op: &Op) -> Op {
        let mut r = op.clone();
        let own = r.allocates().map(str::to_string);
        r.for_each_ref(|x| {
            if own.as_deref() != Some(x.as_str())
                && let Some(id) = self.bound.get(x.as_str())
            {
                *x = id.clone();
            }
        });
        r
    }

    /// Realises `g` and applies it; `Some(op)` when it applied.
    pub fn realise(&mut self, g: &G) -> Option<Op> {
        let op = self.op(g)?;
        if op.is_internal() && !self.internal {
            return None;
        }
        let mode = if op.is_internal() {
            ApplyMode::Internal
        } else {
            ApplyMode::Client
        };
        let mut ids = self.ids.clone();
        let a = apply_tx_mode(&self.state, &tx(vec![self.resolved(&op)]), &mut ids, mode).ok()?;
        if let Op::ItemRemove(r) = &op
            && let Some((item, track)) = self.find(&self.resolved(&op), &r.id)
        {
            self.graveyard.push((item, track));
        }
        self.ids = ids;
        self.bound.extend(a.bindings);
        self.state = a.project;
        self.ops.push(op.clone());
        Some(op)
    }

    fn find(&self, _op: &Op, id: &str) -> Option<(Item, String)> {
        let id = self.bound.get(id).map_or(id, String::as_str);
        self.state
            .timelines
            .iter()
            .flat_map(|tl| &tl.tracks)
            .find_map(|t| {
                t.items
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| (c.clone(), t.id.clone()))
            })
    }

    /// Realises every abstract op in turn; returns the ops that applied.
    pub fn run(&mut self, gs: &[G]) -> Vec<Op> {
        gs.iter().filter_map(|g| self.realise(g)).collect()
    }

    fn op(&mut self, g: &G) -> Option<Op> {
        Some(match *g {
            G::Insert {
                t,
                k,
                off,
                len,
                overwrite,
                kind,
            } => self.insert_op(t, k, off, len, overwrite, kind)?,
            G::Trim {
                t,
                k,
                start,
                n,
                mode,
            } => {
                let it = self.item(t, k)?;
                ItemTrim {
                    item: self.key(&it.id),
                    edge: if start { Edge::Start } else { Edge::End },
                    delta: frames(n),
                    mode: [
                        TrimMode::Ripple,
                        TrimMode::Roll,
                        TrimMode::Overwrite,
                        TrimMode::Strict,
                    ][mode as usize % 4],
                    alone: false,
                }
                .into()
            }
            G::Delete { t, k, lift } => ItemDelete {
                item: self.key(&self.item(t, k)?.id),
                mode: mode_of(lift),
            }
            .into(),
            G::Remove { t, k, lift } => ItemRemove {
                id: self.key(&self.item(t, k)?.id),
                mode: mode_of(lift),
            }
            .into(),
            G::Restore { k, fresh, lift } => {
                let (mut item, track) = pick(&self.graveyard, k)?.clone();
                // Alone: a restore into a group is an undo's, with the link op that rebuilds
                // it; without it a stale group id makes a one-member group no edit can reach.
                item.common.link = None;
                if fresh {
                    item.id = self.name("r");
                }
                ItemRestore {
                    item,
                    track,
                    mode: mode_of(lift),
                }
                .into()
            }
            G::Slip { t, k, n } => ItemSlip {
                item: self.key(&self.item(t, k)?.id),
                delta: frames(n),
            }
            .into(),
            G::Split { t, k, eighths } => {
                let it = self.item(t, k)?;
                let n = (it.duration.num() * 30 / it.duration.den()) * eighths / 8;
                ItemSplit {
                    item: self.key(&it.id),
                    offset: frames(n),
                    id: self.name("s"),
                }
                .into()
            }
            G::Move {
                t,
                k,
                to,
                at,
                off,
                overwrite,
            } => {
                let it = self.item(t, k)?;
                let base = match at.and_then(|a| self.item(to, a)) {
                    Some(c) => c.start,
                    None => self.end_of(to)?,
                };
                ItemMove {
                    item: self.key(&it.id),
                    track: TRACKS[to % 4].into(),
                    at: (base + frames(off)).ok()?,
                    mode: if overwrite {
                        InsertMode::Overwrite
                    } else {
                        InsertMode::Ripple
                    },
                }
                .into()
            }
            G::MoveLinked { k, to, at } => {
                let linked: Vec<&Item> = self
                    .all_items()
                    .into_iter()
                    .filter(|i| i.common.link.is_some())
                    .collect();
                let it = (*pick(&linked, k)?).clone();
                let base = match at.and_then(|a| self.item(to, a)) {
                    Some(c) => c.start,
                    None => self.end_of(to)?,
                };
                ItemMove {
                    item: self.key(&it.id),
                    track: TRACKS[to % 4].into(),
                    at: base,
                    mode: InsertMode::Ripple,
                }
                .into()
            }
            G::Slide { t, k, n } => ItemSlide {
                item: self.key(&self.item(t, k)?.id),
                delta: frames(n),
                sides: None,
            }
            .into(),
            G::Link { a, b } => {
                let (x, y) = (self.item(a.0, a.1)?, self.item(b.0, b.1)?);
                ItemLink {
                    items: vec![self.key(&x.id), self.key(&y.id)],
                    id: self.name("lk"),
                }
                .into()
            }
            G::Unlink { t, k } => ItemUnlink {
                group: self.key(&self.item(t, k)?.common.link?),
                items: None,
            }
            .into(),
            G::TrSet { t, k, n } => {
                let track = self.track(t)?;
                let items = &track.items;
                if items.len() < 2 {
                    return None;
                }
                let i = k % (items.len() - 1);
                let between = [self.key(&items[i].id), self.key(&items[i + 1].id)];
                let existing = track
                    .transitions
                    .iter()
                    .find(|x| x.between == [items[i].id.clone(), items[i + 1].id.clone()])
                    .map(|x| self.key(&x.id));
                let (track_id, kind) = (track.id.clone(), tr_kind(track));
                TransitionSet {
                    id: existing.unwrap_or_else(|| self.name("tr")),
                    track: track_id,
                    between,
                    kind,
                    duration: frames(n),
                    curve: if n % 2 == 0 {
                        Curve::Linear
                    } else {
                        Curve::EqualPower
                    },
                }
                .into()
            }
            G::TrRemove { t, k } => TransitionRemove {
                transition: self.key(&pick(&self.track(t)?.transitions, k)?.id),
            }
            .into(),
            G::Param { t, k, which, v } => {
                let it = self.item(t, k)?;
                let (path, value) = match which {
                    0 => ("opacity", None),
                    1 => (
                        "opacity",
                        Some(ParamValue::Number((v.abs() % 10) as f64 / 10.0)),
                    ),
                    2 => (
                        "transform.rotation",
                        Some(ParamValue::Number(v as f64 * 9.0)),
                    ),
                    3 => ("audio.gain", Some(ParamValue::Number(-(v.abs() as f64)))),
                    4 => (
                        "crop.left",
                        Some(ParamValue::Number((v.abs() % 5) as f64 / 10.0)),
                    ),
                    5 => (
                        "transform.position",
                        Some(ParamValue::Vec2([v as f64 * 10.0, 0.5])),
                    ),
                    _ => ("blend", Some(ParamValue::Text("multiply".into()))),
                };
                ParamSet {
                    item: self.key(&it.id),
                    path: path.into(),
                    value,
                }
                .into()
            }
            G::EffectAdd { t, k, kind, at } => {
                let it = self.item(t, k)?;
                let mut params = BTreeMap::new();
                let kind = match kind {
                    0 => "blur",
                    1 => "colorAdjust",
                    _ => {
                        let lut = self
                            .state
                            .assets
                            .values()
                            .find(|a| a.kind == AssetKind::Lut)?;
                        params.insert(
                            "asset".to_string(),
                            ParamValue::Asset {
                                asset: self.key(&lut.id),
                            },
                        );
                        "lut"
                    }
                };
                EffectAdd {
                    item: self.key(&it.id),
                    id: self.name("fx"),
                    kind: kind.into(),
                    at,
                    params,
                }
                .into()
            }
            G::EffectRemove { t, k, e } => {
                let it = self.with_effects(t, k)?;
                EffectRemove {
                    item: self.key(&it.id),
                    effect: self.key(&pick(&it.common.effects, e)?.id),
                }
                .into()
            }
            G::EffectMove { t, k, e, to } => {
                let it = self.with_effects(t, k)?;
                let n = it.common.effects.len();
                let from = e % n;
                EffectMove {
                    item: self.key(&it.id),
                    effect: self.key(&it.common.effects[from].id),
                    to: ((from + 1 + to as usize) % n) as u32,
                }
                .into()
            }
            G::KeySet {
                t,
                k,
                which,
                at,
                v,
                interp,
            } => {
                let it = self.item(t, k)?;
                let path = match which {
                    0 => "opacity".to_string(),
                    1 => {
                        let fx = it.common.effects.first()?;
                        let param = match fx.kind.as_str() {
                            "blur" => "radius",
                            "colorAdjust" => "exposure",
                            _ => "intensity",
                        };
                        format!("effects.{}.{param}", self.key(&fx.id))
                    }
                    2 => "transform.rotation".to_string(),
                    _ => "audio.gain".to_string(),
                };
                let value = match which {
                    0 => (v % 10) as f64 / 10.0,
                    1 => (v % 2) as f64 / 2.0,
                    2 => v as f64 * 5.0,
                    _ => -(v as f64),
                };
                let n = it.duration.num() * 30 / it.duration.den();
                KeyframeSet {
                    item: self.key(&it.id),
                    path,
                    at: frames(at % n.max(1)),
                    value: ParamValue::Number(value),
                    interp: match interp {
                        0 => Interp::Linear,
                        1 => Interp::Hold,
                        _ => Interp::Bezier {
                            x1: 0.25,
                            y1: 0.1,
                            x2: 0.25,
                            y2: 1.0,
                        },
                    },
                }
                .into()
            }
            G::KeyRemove { t, k, j } => {
                let it = self.item(t, k)?;
                let all: Vec<(&String, Rational)> = it
                    .common
                    .keyframes
                    .iter()
                    .flat_map(|(p, ks)| ks.iter().map(move |x| (p, x.at)))
                    .collect();
                let (path, at) = pick(&all, j)?;
                let path = match fin_core::params::effect_path(path) {
                    Some((fx, rest)) => format!("effects.{}.{rest}", self.key(fx)),
                    None => (*path).clone(),
                };
                KeyframeRemove {
                    item: self.key(&it.id),
                    path,
                    at: *at,
                }
                .into()
            }
            G::Caption { k, what, x } => self.caption_op(k, what, x)?,
            G::StyleAdd { from, v } => {
                let from = match from {
                    0 => None,
                    1 => Some("preset:clean".to_string()),
                    _ => self.state.styles.keys().next().map(|s| self.key(s)),
                };
                StyleAdd {
                    id: self.name("st"),
                    name: format!("S{v}"),
                    from,
                    set: BTreeMap::from([("text.size".into(), ParamValue::Number(v as f64))]),
                }
                .into()
            }
            G::StyleSet { k, which, v } => {
                let styles: Vec<&String> = self.state.styles.keys().collect();
                let st = self.key(pick(&styles, k)?);
                let (path, value) = match which {
                    0 => ("text.size", Some(ParamValue::Number(v as f64))),
                    1 => ("text.size", None),
                    2 => (
                        "background.padding",
                        Some(ParamValue::Number(v as f64 / 4.0)),
                    ),
                    _ => ("text.color", Some(ParamValue::Text("#ff0000".into()))),
                };
                StyleSet {
                    style: st,
                    path: path.into(),
                    value,
                }
                .into()
            }
            G::StyleRemove { k } => {
                let styles: Vec<&String> = self.state.styles.keys().collect();
                StyleRemove {
                    style: self.key(pick(&styles, k)?),
                }
                .into()
            }
            G::FontPin { which, fallback } => match which {
                0 => pin(INTER, inter(&sha('a')), fallback),
                1 => pin(INTER, inter(&sha('b')), fallback),
                2 => pin(NOTO, catalog_pin("Noto Sans", 400, &sha('d')), fallback),
                _ => {
                    let font = self
                        .state
                        .assets
                        .values()
                        .find(|a| a.kind == AssetKind::Font)?;
                    pin(TEST_SANS, asset_pin(&self.key(&font.id)), fallback)
                }
            },
            G::FontUnpin { k } => {
                let keys: Vec<&String> = self.state.fonts.pins.keys().collect();
                FontUnpin::new(pick(&keys, k)?.as_str()).into()
            }
            G::FontRepin { k, c } => {
                let keys: Vec<&String> = self.state.fonts.pins.keys().collect();
                let key = (*pick(&keys, k)?).clone();
                let mut p = self.state.fonts.pins[&key].clone();
                p.sha256 = sha(['a', 'b', 'e'][c as usize % 3]);
                FontRepin { key, pin: p }.into()
            }
            G::Track { what, k, x } => self.track_op(what, k, x)?,
            G::Timeline { what, k } => self.timeline_op(what, k)?,
            G::Marker { what, k, at } => self.marker_op(what, k, at)?,
            G::Bin { what, k, x } => self.bin_op(what, k, x)?,
            G::Asset { what, k, x } => self.asset_op(what, k, x)?,
        })
    }

    fn insert_op(
        &mut self,
        t: usize,
        k: Option<usize>,
        off: i64,
        len: i64,
        overwrite: bool,
        kind: u8,
    ) -> Option<Op> {
        let at = match k.and_then(|k| self.item(t, k)) {
            Some(c) => (c.start + frames(off.max(0))).ok()?,
            None => (self.end_of(t)? + frames(off.max(0) % 3)).ok()?,
        };
        let track = TRACKS[t % 4];
        if kind == 2 || track == "t3" {
            let texts: [&[&str]; 3] = [&["hi", "there"], &["a", "b", "c"], &["yo"]];
            let mut op = caption(
                &self.name("cap"),
                track,
                at,
                texts[(len % 3) as usize],
                "preset:clean",
            );
            if let Op::ItemInsert(i) = &mut op
                && !overwrite
            {
                i.mode = InsertMode::Ripple;
            }
            return Some(op);
        }
        let a1 = self
            .state
            .assets
            .values()
            .find(|a| a.kind == AssetKind::Media && a.video.is_some() && a.audio.is_some())?;
        let body = match kind {
            1 => ItemBody::Adjustment {},
            3 => ItemBody::Nested {
                timeline: self.key(&self.state.timelines.get(1)?.id),
                source_in: Rational::zero(),
            },
            _ => clip_body(&self.key(&a1.id), secs(10)),
        };
        Some(
            ItemInsert {
                id: self.name("i"),
                track: track.into(),
                at,
                duration: frames(len),
                mode: if overwrite {
                    InsertMode::Overwrite
                } else {
                    InsertMode::Ripple
                },
                body,
            }
            .into(),
        )
    }

    fn caption_op(&mut self, k: usize, what: u8, x: usize) -> Option<Op> {
        let caps = &self.track(2)?.items;
        let it = pick(caps, k)?.clone();
        let next = caps
            .iter()
            .position(|c| c.id == it.id)
            .and_then(|i| caps.get(i + 1))
            .cloned();
        let item = self.key(&it.id);
        Some(match what {
            0 => CaptionSetText {
                item,
                text: ["one two", "one more two", "new", "three four"][x % 4].into(),
            }
            .into(),
            1 => CaptionSplit {
                item,
                word: 1 + x as u32 % 2,
                id: self.name("cap"),
            }
            .into(),
            2 => CaptionMerge {
                item,
                with: self.key(&next?.id),
            }
            .into(),
            _ => {
                let styles: Vec<&String> = self.state.styles.keys().collect();
                let style = match pick(&styles, x) {
                    Some(s) if x.is_multiple_of(2) => self.key(s),
                    _ => "preset:punch".into(),
                };
                CaptionSetStyle {
                    item,
                    style,
                    overrides: (x == 3)
                        .then(|| BTreeMap::from([("text.size".into(), ParamValue::Number(60.0))])),
                }
                .into()
            }
        })
    }

    fn track_op(&mut self, what: u8, k: usize, x: usize) -> Option<Op> {
        let tl = self.state.timelines[0].id.clone();
        let tracks: Vec<String> = self.state.timelines[0]
            .tracks
            .iter()
            .map(|t| self.key(&t.id))
            .collect();
        let tr = pick(&tracks, k)?.clone();
        let n = tracks.len() as u32;
        Some(match what {
            0 => TrackAdd {
                id: self.name("t"),
                timeline: tl,
                kind: [TrackKind::Video, TrackKind::Audio, TrackKind::Caption][x % 3],
                index: (x.is_multiple_of(2)).then_some(x as u32 % (n + 1)),
                name: None,
            }
            .into(),
            1 => TrackRemove {
                track: tr.clone(),
                with_items: x.is_multiple_of(2),
            }
            .into(),
            2 => TrackMove {
                track: tr.clone(),
                index: x as u32 % n,
            }
            .into(),
            _ => {
                let (path, value) = match x % 4 {
                    0 => ("mute", Some(ParamValue::Bool(true))),
                    1 => ("gain", Some(ParamValue::Number(-3.0))),
                    2 => ("name", Some(ParamValue::Text("Renamed".into()))),
                    _ => ("solo", None),
                };
                TrackSet {
                    track: tr.clone(),
                    path: path.into(),
                    value,
                }
                .into()
            }
        })
    }

    fn timeline_op(&mut self, what: u8, k: usize) -> Option<Op> {
        let tls: Vec<String> = self.state.timelines.iter().map(|t| t.id.clone()).collect();
        let tl = self.key(pick(&tls, k)?);
        Some(match what {
            0 => TimelineAdd {
                id: self.name("tl"),
                name: "Second".into(),
                rate: secs(30),
                resolution: Resolution {
                    width: 1080,
                    height: 1920,
                },
            }
            .into(),
            1 => TimelineSet {
                timeline: tl,
                path: "name".into(),
                value: Some(ParamValue::Text("Main".into())),
            }
            .into(),
            _ => TimelineRemove { timeline: tl }.into(),
        })
    }

    fn marker_op(&mut self, what: u8, k: usize, at: i64) -> Option<Op> {
        let markers: Vec<String> = self.state.timelines[0]
            .markers
            .iter()
            .map(|m| m.id.clone())
            .collect();
        Some(match what {
            0 => MarkerAdd {
                id: self.name("m"),
                timeline: self.state.timelines[0].id.clone(),
                at: frames(at),
                duration: (at % 2 == 0).then(|| frames(15)),
                label: "beat".into(),
                color: None,
            }
            .into(),
            1 => MarkerSet {
                marker: self.key(pick(&markers, k)?),
                path: "label".into(),
                value: Some(ParamValue::Text("drop".into())),
            }
            .into(),
            _ => MarkerRemove {
                marker: self.key(pick(&markers, k)?),
            }
            .into(),
        })
    }

    fn bin_op(&mut self, what: u8, k: usize, x: usize) -> Option<Op> {
        let bins: Vec<String> = self.state.bins.keys().cloned().collect();
        Some(match what {
            0 => BinAdd {
                id: self.name("bin"),
                name: "Footage".into(),
                parent: pick(&bins, x)
                    .filter(|_| x.is_multiple_of(2))
                    .map(|b| self.key(b)),
            }
            .into(),
            1 => BinMove {
                bin: self.key(pick(&bins, k)?),
                parent: pick(&bins, x)
                    .filter(|_| x.is_multiple_of(2))
                    .map(|b| self.key(b)),
            }
            .into(),
            _ => BinRemove {
                bin: self.key(pick(&bins, k)?),
            }
            .into(),
        })
    }

    fn asset_op(&mut self, what: u8, k: usize, x: usize) -> Option<Op> {
        let assets: Vec<String> = self.state.assets.keys().cloned().collect();
        let bins: Vec<String> = self.state.bins.keys().cloned().collect();
        Some(match what {
            0 => av_asset(&self.name("a"), secs(20 + x as i64)),
            1 => lut_asset(&self.name("a")),
            2 => font_asset(&self.name("a")),
            3 => Op::AssetRemove {
                id: self.key(pick(&assets, k)?),
            },
            _ => AssetMove {
                asset: self.key(pick(&assets, k)?),
                bin: pick(&bins, x).map(|b| self.key(b)),
            }
            .into(),
        })
    }
}

fn mode_of(lift: bool) -> DeleteMode {
    if lift {
        DeleteMode::Lift
    } else {
        DeleteMode::Ripple
    }
}

/// Every op name a client or internal tx may hold (aliases excluded).
pub const OP_NAMES: &[&str] = &[
    "asset.add",
    "asset.move",
    "asset.remove",
    "bin.add",
    "bin.move",
    "bin.remove",
    "caption.merge",
    "caption.setStyle",
    "caption.setText",
    "caption.setWords",
    "caption.split",
    "effect.add",
    "effect.move",
    "effect.remove",
    "effect.restore",
    "font.pin",
    "font.repin",
    "font.unpin",
    "item.delete",
    "item.insert",
    "item.link",
    "item.move",
    "item.remove",
    "item.restore",
    "item.slide",
    "item.slip",
    "item.split",
    "item.trim",
    "item.unlink",
    "keyframe.remove",
    "keyframe.set",
    "marker.add",
    "marker.remove",
    "marker.set",
    "param.set",
    "style.add",
    "style.remove",
    "style.set",
    "timeline.add",
    "timeline.remove",
    "timeline.restore",
    "timeline.set",
    "track.add",
    "track.move",
    "track.remove",
    "track.restore",
    "track.set",
    "transition.remove",
    "transition.set",
];
