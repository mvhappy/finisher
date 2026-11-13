//! Range, kind and structural checks shared by the op modules.

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Asset, AssetKind, Item, ItemBody, Project, Track, TrackKind};
use crate::ops::Bindings;
use crate::ops::util::{TrackAt, end};
use crate::params::{self, EffectDomain};
use crate::time::Rational;

pub(crate) fn invalid_range(
    message: impl Into<String>,
    details: serde_json::Value,
    fix: impl Into<String>,
) -> CoreError {
    CoreError::new("INVALID_RANGE", message)
        .with_details(details)
        .with_fix(fix)
}

/// A clip's asset must be a `media` asset carrying the stream the track plays (structural).
pub(crate) fn check_kind(track: &Track, asset: &Asset) -> Result<(), CoreError> {
    let ok = asset.kind == AssetKind::Media
        && match track.kind {
            TrackKind::Video => asset.video.is_some(),
            TrackKind::Audio => asset.audio.is_some(),
            TrackKind::Caption => false,
        };
    if ok {
        return Ok(());
    }
    let want = kind_name(track.kind);
    Err(CoreError::new(
        "TRACK_KIND_MISMATCH",
        format!(
            "asset {} has no {want} stream for track {}",
            asset.id, track.id
        ),
    )
    .with_details(json!({ "track": track.id, "trackKind": want, "asset": asset.id }))
    .with_fix(format!(
        "put the asset on a track whose kind it has a stream for, not {want}"
    )))
}

/// Policy (plan 6C Decision 13): every registry effect in `kinds` (of `item`) has the domain
/// of `track`: visual effects on items of video (and caption) tracks, audio effects on clips of
/// audio tracks; otherwise `TRACK_KIND_MISMATCH` for the first that does not. An audio effect on
/// an item that is not a clip (a nested item) is `INVALID_OP`: the mixer runs effects per clip
/// and a container's own effects are not in the audio plan until nested-bus processing exists
/// (compressing a sum is not compressing its parts). Unknown kinds have no domain and pass.
/// Shared by every op that puts an effect on an item or an item with effects on a track:
/// `effect.add`, `effect.restore`, `item.move`, `item.restore` and track restores. Skipped on
/// replay.
pub(crate) fn check_effect_domains<'a>(
    track: &Track,
    item: &Item,
    kinds: impl IntoIterator<Item = &'a str>,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    if !bindings.policy() {
        return Ok(());
    }
    let audio_track = track.kind == TrackKind::Audio;
    let id = &item.id;
    for kind in kinds {
        let (what, fix) = match params::effect_domain(kind) {
            Some(EffectDomain::Visual) if audio_track => {
                ("a visual", "add visual effects to items on video tracks")
            }
            Some(EffectDomain::Audio) if !audio_track => {
                ("an audio", "add audio effects to clips on audio tracks")
            }
            Some(EffectDomain::Audio) if !matches!(item.body, ItemBody::Clip { .. }) => {
                let item_kind = item.kind_name();
                return Err(CoreError::new(
                    "INVALID_OP",
                    format!(
                        "{kind} is an audio effect; item {id} is a {item_kind} item, not a clip"
                    ),
                )
                .with_details(json!({ "item": id, "kind": kind, "itemKind": item_kind }))
                .with_fix(
                    "add audio effects to clips; effects on nested items are not applied yet",
                ));
            }
            _ => continue,
        };
        let shown = kind_name(track.kind);
        return Err(CoreError::new(
            "TRACK_KIND_MISMATCH",
            format!(
                "{kind} is {what} effect; item {id} is on {shown} track {}",
                track.id
            ),
        )
        .with_details(json!({ "item": id, "kind": kind, "track": track.id, "trackKind": shown }))
        .with_fix(fix));
    }
    Ok(())
}

/// The wire name of a track kind.
pub(crate) fn kind_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Caption => "caption",
    }
}

/// The length of a media asset (`INVALID_OP` for an asset without one).
pub(crate) fn media_duration(asset: &Asset) -> Result<Rational, CoreError> {
    asset.duration.ok_or_else(|| {
        CoreError::new(
            "INVALID_OP",
            format!(
                "asset {} has no duration: it is not a media asset",
                asset.id
            ),
        )
        .with_details(json!({ "asset": asset.id }))
        .with_fix("use a media asset")
    })
}

/// At least one `frame` (of the item's timeline) long (policy).
pub(crate) fn check_frame(frame: Rational, duration: Rational) -> Result<(), CoreError> {
    if duration < frame {
        return Err(invalid_range(
            "duration must be at least one frame",
            json!({ "duration": duration, "min": frame }),
            format!("use a duration of at least {frame}s"),
        ));
    }
    Ok(())
}

/// A clip's speed is positive (structural: source times would run backwards or stand still).
pub(crate) fn check_speed(speed: Rational) -> Result<(), CoreError> {
    if speed.is_positive() {
        return Ok(());
    }
    Err(invalid_range(
        "speed must be positive",
        json!({ "speed": speed, "min": "0", "exclusive": true }),
        "use a speed greater than 0, e.g. 1 or 1/2",
    ))
}

/// `source_in >= 0`, `duration >= frame`, and the source extent `duration × speed`
/// (plan 4C Decision 14) ends within the asset: `source_in + duration × speed <=
/// asset.duration`.
pub(crate) fn check_source(
    frame: Rational,
    asset: &Asset,
    source_in: Rational,
    duration: Rational,
    speed: Rational,
) -> Result<(), CoreError> {
    if source_in < Rational::zero() {
        return Err(invalid_range(
            "sourceIn must not be negative",
            json!({ "sourceIn": source_in, "min": Rational::zero() }),
            "use a sourceIn of 0 or more",
        ));
    }
    check_frame(frame, duration)?;
    let length = media_duration(asset)?;
    if source_in.checked_add(duration.checked_mul(speed)?)? > length {
        let max = length.checked_sub(source_in)?.checked_div(speed)?;
        return Err(invalid_range(
            format!(
                "sourceIn + duration runs past the end of asset {}",
                asset.id
            ),
            json!({ "sourceIn": source_in, "duration": duration, "speed": speed,
                    "assetDuration": length, "max": max }),
            format!("use a duration of at most {max}s from this sourceIn"),
        ));
    }
    Ok(())
}

/// The structural invariant of a track: positive durations (`CLIP_EMPTY`), non-negative
/// `start` and `sourceIn` (`CLIP_NEGATIVE`), sorted by `start` with no overlaps
/// (`CLIP_OVERLAP`). Policy checks keep it on the edit path; replay, which skips them, checks it
/// after every op on each track the op touched ([`crate::ops::util::touched_tracks`]).
///
/// It checks each track's transitions too ([`crate::ops::util::check_transitions`],
/// `TRANSITION_INVALID`); [`check_track`], which ops run mid-edit, before the prune pass,
/// does not.
pub(crate) fn check_tracks(project: &Project, tracks: &[TrackAt]) -> Result<(), CoreError> {
    for t in tracks {
        check_track(t.get(project))?;
        crate::ops::util::check_transitions(t.get(project))?;
    }
    Ok(())
}

/// [`check_tracks`] for one track.
pub(crate) fn check_track(track: &Track) -> Result<(), CoreError> {
    for c in &track.items {
        if !c.duration.is_positive() {
            return Err(CoreError::new(
                "CLIP_EMPTY",
                format!("item {} has no positive duration", c.id),
            )
            .with_details(
                json!({ "track": track.id, "clip": c.id, "item": c.id, "duration": c.duration }),
            ));
        }
        let source_in = c.source_in().unwrap_or_default();
        if c.start < Rational::zero() || source_in < Rational::zero() {
            return Err(CoreError::new(
                "CLIP_NEGATIVE",
                format!("item {} has a negative start or sourceIn", c.id),
            )
            .with_details(json!({
                "track": track.id,
                "clip": c.id,
                "item": c.id,
                "start": c.start,
                "sourceIn": source_in,
            })));
        }
    }
    for w in track.items.windows(2) {
        let prev_end = end(&w[0])?;
        if w[1].start < prev_end {
            return Err(CoreError::new(
                "CLIP_OVERLAP",
                format!(
                    "items {} and {} overlap on track {}",
                    w[0].id, w[1].id, track.id
                ),
            )
            .with_details(json!({
                "track": track.id,
                "clips": [w[0].id, w[1].id],
                "end": prev_end,
                "start": w[1].start,
            })));
        }
    }
    Ok(())
}
