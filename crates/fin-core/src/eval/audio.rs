//! `eval_audio`: what the mixer plays over a range (sub-project 4 spec §5.4, plan 4C Decision
//! 41). Gains stay in dB (no `pow` in `fin-core`); the mixer turns them and the shapes into
//! amplitudes.
//!
//! - Only audio tracks sound. A muted track is omitted; when any audio track is soloed, only
//!   soloed ones sound.
//! - Clip items sound over their extent with transitions (`[cut − d/2, cut + d/2)` extends A
//!   and starts B early); other kinds on audio tracks are silent in 4C.
//! - Gain is the track gain plus the item's `audio.gain` (keyframed in dB), times the item's
//!   shapes: `fadeIn` over the first `fadeIn` of the extent, `fadeOut` over the last
//!   `fadeOut`, and a crossfade's `crossOut`/`crossIn` over its window with its curve.
//! - Gain segments break at gain keyframes, fade ends and transition edges; pan segments at
//!   pan keyframes; both are clipped to `[from, to)`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::eval::segments::{NumberCurve, breakpoints};
use crate::eval::source::clip_time;
use crate::eval::transition::{Window, extent, roles, windows};
use crate::eval::{check_audio, timeline_of};
use crate::model::{AudioParams, Curve, Interp, Item, ItemBody, Project, Track, TrackKind};
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
/// timeline span `[from, to)`. `gain` and `pan` cover that span without gaps, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioItem {
    pub item: String,
    pub track: String,
    pub asset: String,
    pub from: Rational,
    pub to: Rational,
    pub source_from: Rational,
    pub speed: Rational,
    pub gain: Vec<GainSegment>,
    pub pan: Vec<PanSegment>,
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
    let audio = |t: &&Track| t.kind == TrackKind::Audio;
    let solo = tl.tracks.iter().filter(audio).any(|t| t.solo);
    let mut items = Vec::new();
    for track in tl.tracks.iter().filter(audio) {
        if track.mute || (solo && !track.solo) {
            continue;
        }
        let windows = windows(track)?;
        for (i, item) in track.items.iter().enumerate() {
            if let Some(a) = audio_item(track, &windows, i, item, from, to)? {
                items.push(a);
            }
        }
    }
    let plan = AudioPlan {
        timeline: tl.id.clone(),
        from,
        to,
        items,
    };
    check_audio(&plan)?;
    Ok(plan)
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
    windows: &[Window],
    i: usize,
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
    let (into, out) = roles(windows, i);
    let (start, end) = extent(item, into, out)?;
    let (a, b) = (start.max(from), end.min(to));
    if a >= b {
        return Ok(None);
    }
    let params = item.common.audio.clone().unwrap_or_default();
    let shapes = shape_windows(&params, start, end, into, out)?;
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
        asset: asset.clone(),
        from: a,
        to: b,
        source_from: clip_time(*source_in, *speed, a.checked_sub(item.start)?)?,
        speed: *speed,
        gain: gain_segments,
        pan: pan_segments,
    }))
}

/// The fades (over the extent's first and last stretch; linear) and the crossfade sides.
fn shape_windows(
    params: &AudioParams,
    start: Rational,
    end: Rational,
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
    if params.fade_in.is_positive() {
        let e = start.checked_add(params.fade_in)?;
        push(ShapeKind::FadeIn, Curve::Linear, start, e);
    }
    if params.fade_out.is_positive() {
        let s = end.checked_sub(params.fade_out)?;
        push(ShapeKind::FadeOut, Curve::Linear, s, end);
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
