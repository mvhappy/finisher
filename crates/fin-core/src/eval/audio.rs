//! `eval_audio`: what the mixer plays over a range (sub-project 4 spec §5.4, plan 4C Decision
//! 41). Gains stay in dB (no `pow` in `fin-core`); the mixer turns them and the shapes into
//! amplitudes.
//!
//! - Audio tracks sound their clips; nested items sound their timeline's audio, on any track.
//!   A muted track is omitted; when any track that sounds (an audio track, or one holding a
//!   nested item) is soloed, only soloed ones sound. Mute and solo apply per timeline: a nested
//!   timeline's own flags decide among its tracks.
//! - Clip items sound over their extent with transitions (`[cut − d/2, cut + d/2)` extends A
//!   and starts B early); other kinds on audio tracks are silent in 4C.
//! - Gain is the track gain plus the item's `audio.gain` (keyframed in dB; only number
//!   keyframes count), times the item's shapes: `fadeIn` over the item's first `fadeIn`,
//!   `fadeOut` over its last `fadeOut`, and a crossfade's `crossOut`/`crossIn` over its window
//!   with its curve. A side with a crossfade has no fade (never a double fade).
//! - A nested item's audio is its timeline's plan over the matching range (`sourceIn` on),
//!   clipped to the nested item's extent and moved onto this timeline, with the nested item's
//!   track gain added; its items list the nested items they sound through (`via`). A missing
//!   timeline is silent; a cycle is `NESTED_CYCLE`, and more than
//!   [`super::MAX_NESTED_LAYERS`] nested items in all `NESTED_TOO_LARGE`.
//! - Gain segments break at gain keyframes, fade ends and transition edges; pan segments at
//!   pan keyframes; both are clipped to `[from, to)`.
//! - Each clip's enabled audio effects come with it ([`super::audio_effects`]); a nested
//!   item's are shifted with its gain and pan. A container's own effects (a nested item's) are
//!   not in the plan: effects run per clip, and compressing a sum is not compressing its
//!   parts; `effect.add` refuses audio effects on anything but a clip until nested-bus
//!   processing exists.
//! - Each track is indexed once ([`TrackIndex`]) and only the items that can meet the range
//!   are visited.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::eval::MAX_NESTED_LAYERS;
use crate::eval::audio_effects::{AudioEffectPlan, effects_of};
use crate::eval::frame::{nested_cycle, nested_too_large};
use crate::eval::segments::{NumberCurve, breakpoints};
use crate::eval::source::clip_time;
use crate::eval::transition::{TrackIndex, Window, extent};
use crate::eval::{MAX_NESTING, check_audio, timeline_of};
use crate::model::{
    AudioParams, Curve, Interp, Item, ItemBody, Project, Timeline, Track, TrackKind,
};
use crate::time::Rational;

/// The items sounding in `[from, to)` of one timeline, in track then time order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioPlan {
    pub timeline: String,
    pub from: Rational,
    pub to: Rational,
    pub items: Vec<AudioItem>,
}

/// One item's part of the range: it plays `asset` from `sourceFrom` at `speed` over the
/// timeline span `[from, to)`. `gain` and `pan` cover that span without gaps, in order. An
/// item of a nested timeline lists the nested items it sounds through in `via`, outermost
/// first (its `item` and `track` are its own, on the nested timeline). `effects` are its
/// enabled effects with their numbers as segments over the same span.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioItem {
    pub item: String,
    pub track: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<String>,
    pub asset: String,
    pub from: Rational,
    pub to: Rational,
    pub source_from: Rational,
    pub speed: Rational,
    pub gain: Vec<GainSegment>,
    pub pan: Vec<PanSegment>,
    /// The item's enabled audio effects in its order (plan 6C Decision 15); absent when none,
    /// so plans without effects hash as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<AudioEffectPlan>,
}

/// Gain over `[from, to)` in dB: `db` are the end values of the keyframe span the segment
/// lies in, `u` the segment's progress range through that span, so the dB at progress `x` of
/// the segment is `lerp(db[0], db[1], ease(lerp(u[0], u[1], x)))` with `ease` from `interp`
/// (`hold`: `db[0]` throughout). The `shapes` multiply the amplitude.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GainSegment {
    pub from: Rational,
    pub to: Rational,
    pub db: [f64; 2],
    pub interp: Interp,
    pub u: [f64; 2],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<Shape>,
}

/// A fade or crossfade side over a segment: `p` is its progress at the segment's ends (0 → 1
/// across the whole fade). Its amplitude factor is `curve(p)` for `fadeIn`/`crossIn` and
/// `curve(1 − p)` for `fadeOut`/`crossOut` (`linear`: `x`; `equalPower`: `sqrt(x)`-shaped, as
/// the mixer defines it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Shape {
    pub kind: ShapeKind,
    pub curve: Curve,
    pub p: [f64; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ShapeKind {
    FadeIn,
    FadeOut,
    CrossIn,
    CrossOut,
}

/// Pan over `[from, to)` (−1 left … 1 right; the mixer clamps), read like [`GainSegment`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PanSegment {
    pub from: Rational,
    pub to: Rational,
    pub pan: [f64; 2],
    pub interp: Interp,
    pub u: [f64; 2],
}

/// The audio plan of `timeline` over `[from, to)`. Errors: `INVALID_RANGE` (`from ≥ to`),
/// `NOT_FOUND` (the timeline), `TIME_OVERFLOW`, and `INTERNAL` should a value come out
/// non-finite.
pub fn eval_audio(
    project: &Project,
    timeline: &str,
    from: Rational,
    to: Rational,
) -> Result<AudioPlan, CoreError> {
    if from >= to {
        return Err(
            CoreError::new("INVALID_RANGE", "the audio range must end after it starts")
                .with_details(json!({ "from": from.to_string(), "to": to.to_string() }))
                .with_fix("pass a range whose end is later than its start"),
        );
    }
    let tl = timeline_of(project, timeline)?;
    let mut mix = Mix {
        project,
        stack: Vec::new(),
        nested: 0,
    };
    let items = mix.items(tl, from, to)?;
    let plan = AudioPlan {
        timeline: tl.id.clone(),
        from,
        to,
        items,
    };
    check_audio(&plan)?;
    Ok(plan)
}

/// One evaluation's walk through nested timelines.
struct Mix<'a> {
    project: &'a Project,
    /// The timelines being evaluated, outermost first.
    stack: Vec<&'a str>,
    /// Nested items sounded so far ([`MAX_NESTED_LAYERS`]).
    nested: usize,
}

fn is_nested(item: &Item) -> bool {
    matches!(item.body, ItemBody::Nested { .. })
}

impl<'a> Mix<'a> {
    /// The items of `tl` sounding in `[from, to)`, in track then time order.
    fn items(
        &mut self,
        tl: &'a Timeline,
        from: Rational,
        to: Rational,
    ) -> Result<Vec<AudioItem>, CoreError> {
        if self.stack.contains(&tl.id.as_str()) || self.stack.len() >= MAX_NESTING {
            return Err(nested_cycle(&self.stack, &tl.id));
        }
        self.stack.push(&tl.id);
        let sounds = |t: &&Track| t.kind == TrackKind::Audio || t.items.iter().any(is_nested);
        let solo = tl.tracks.iter().filter(sounds).any(|t| t.solo);
        let mut items = Vec::new();
        for track in tl.tracks.iter().filter(sounds) {
            if track.mute || (solo && !track.solo) {
                continue;
            }
            let index = TrackIndex::new(track)?;
            for i in index.candidates(from, to, false) {
                let item = &track.items[i];
                let (into, out) = index.roles(i);
                if is_nested(item) {
                    self.nested(track, item, into, out, from, to, &mut items)?;
                } else if track.kind == TrackKind::Audio
                    && let Some(a) = audio_item(track, into, out, item, from, to)?
                {
                    items.push(a);
                }
            }
        }
        self.stack.pop();
        Ok(items)
    }

    /// The audio of nested item `item` (on `track`) over its extent within `[from, to)`.
    #[allow(clippy::too_many_arguments)]
    fn nested(
        &mut self,
        track: &Track,
        item: &Item,
        into: Option<&Window>,
        out: Option<&Window>,
        from: Rational,
        to: Rational,
        items: &mut Vec<AudioItem>,
    ) -> Result<(), CoreError> {
        let ItemBody::Nested {
            timeline,
            source_in,
        } = &item.body
        else {
            return Ok(());
        };
        let (start, end) = extent(item, into, out)?;
        let (a, b) = (start.max(from), end.min(to));
        if a >= b {
            return Ok(());
        }
        let Ok(inner) = timeline_of(self.project, timeline) else {
            return Ok(());
        };
        self.nested += 1;
        if self.nested > MAX_NESTED_LAYERS {
            return Err(nested_too_large(&self.stack));
        }
        // Inner time = sourceIn + (outer − start): outer = inner + offset.
        let offset = item.start.checked_sub(*source_in)?;
        let inner_from = a.checked_sub(offset)?;
        let inner_to = b.checked_sub(offset)?;
        for mut x in self.items(inner, inner_from, inner_to)? {
            x.from = x.from.checked_add(offset)?;
            x.to = x.to.checked_add(offset)?;
            for s in &mut x.gain {
                s.from = s.from.checked_add(offset)?;
                s.to = s.to.checked_add(offset)?;
                s.db = [s.db[0] + track.gain, s.db[1] + track.gain];
            }
            for s in &mut x.pan {
                s.from = s.from.checked_add(offset)?;
                s.to = s.to.checked_add(offset)?;
            }
            for s in x
                .effects
                .iter_mut()
                .flat_map(|e| e.numbers.values_mut().flatten())
            {
                s.from = s.from.checked_add(offset)?;
                s.to = s.to.checked_add(offset)?;
            }
            x.via.insert(0, item.id.clone());
            items.push(x);
        }
        Ok(())
    }
}

/// A shape's window on the timeline.
struct ShapeWindow {
    kind: ShapeKind,
    curve: Curve,
    start: Rational,
    end: Rational,
}

fn audio_item(
    track: &Track,
    into: Option<&Window>,
    out: Option<&Window>,
    item: &Item,
    from: Rational,
    to: Rational,
) -> Result<Option<AudioItem>, CoreError> {
    let ItemBody::Clip {
        asset,
        source_in,
        speed,
    } = &item.body
    else {
        return Ok(None);
    };
    let (start, end) = extent(item, into, out)?;
    let (a, b) = (start.max(from), end.min(to));
    if a >= b {
        return Ok(None);
    }
    let params = item.common.audio.clone().unwrap_or_default();
    let shapes = shape_windows(&params, item, into, out)?;
    let kfs = |path: &str| {
        item.common
            .keyframes
            .get(path)
            .map_or(&[][..], Vec::as_slice)
    };
    let gain = NumberCurve::new(kfs("audio.gain"), params.gain, item.start)?;
    let pan = NumberCurve::new(kfs("audio.pan"), params.pan, item.start)?;

    let edges = shapes.iter().flat_map(|s| [s.start, s.end]);
    let cuts = breakpoints(a, b, gain.cuts().iter().copied().chain(edges));
    let mut gain_segments = Vec::new();
    for w in cuts.windows(2) {
        let (p, q) = (w[0], w[1]);
        let piece = gain.piece(p, q);
        gain_segments.push(GainSegment {
            from: p,
            to: q,
            db: [piece.v[0] + track.gain, piece.v[1] + track.gain],
            interp: piece.interp,
            u: piece.u,
            shapes: shapes_over(&shapes, p, q),
        });
    }
    let cuts = breakpoints(a, b, pan.cuts().iter().copied());
    let pan_segments = cuts
        .windows(2)
        .map(|w| {
            let piece = pan.piece(w[0], w[1]);
            PanSegment {
                from: w[0],
                to: w[1],
                pan: piece.v,
                interp: piece.interp,
                u: piece.u,
            }
        })
        .collect();
    Ok(Some(AudioItem {
        item: item.id.clone(),
        track: track.id.clone(),
        via: Vec::new(),
        asset: asset.clone(),
        from: a,
        to: b,
        source_from: clip_time(*source_in, *speed, a.checked_sub(item.start)?)?,
        speed: *speed,
        gain: gain_segments,
        pan: pan_segments,
        effects: effects_of(item, a, b)?,
    }))
}

/// The fades (linear, from the item's own edges, each window placed past its edge by its
/// offset, [`AudioParams`]) and the crossfade sides. A side with a crossfade has no fade: the
/// crossfade shapes it already. A fade window that does not reach into the item shapes
/// nothing; one that reaches past an edge keeps the progress of the whole window.
fn shape_windows(
    params: &AudioParams,
    item: &Item,
    into: Option<&Window>,
    out: Option<&Window>,
) -> Result<Vec<ShapeWindow>, CoreError> {
    let mut v = Vec::new();
    let mut push = |kind, curve, start, end| {
        v.push(ShapeWindow {
            kind,
            curve,
            start,
            end,
        })
    };
    if params.fade_in.is_positive() && into.is_none() {
        let s = item.start.checked_sub(params.fade_in_offset)?;
        let e = s.checked_add(params.fade_in)?;
        if e > item.start {
            push(ShapeKind::FadeIn, Curve::Linear, s, e);
        }
    }
    if params.fade_out.is_positive() && out.is_none() {
        let end = item.end()?;
        let e = end.checked_add(params.fade_out_offset)?;
        let s = e.checked_sub(params.fade_out)?;
        if s < end {
            push(ShapeKind::FadeOut, Curve::Linear, s, e);
        }
    }
    if let Some(w) = into {
        push(ShapeKind::CrossIn, w.curve, w.start, w.end);
    }
    if let Some(w) = out {
        push(ShapeKind::CrossOut, w.curve, w.start, w.end);
    }
    Ok(v)
}

/// The shapes whose window covers `[p, q)` (a window never starts or ends inside a piece).
fn shapes_over(shapes: &[ShapeWindow], p: Rational, q: Rational) -> Vec<Shape> {
    use crate::eval::interp::progress;
    shapes
        .iter()
        .filter(|s| s.start < q && p < s.end)
        .map(|s| Shape {
            kind: s.kind,
            curve: s.curve,
            p: [progress(s.start, s.end, p), progress(s.start, s.end, q)],
        })
        .collect()
}
