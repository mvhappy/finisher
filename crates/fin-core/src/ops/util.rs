//! Shared lookups, checks and ripple helpers for the op modules.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdKind;
use crate::model::{Asset, Clip, Project, Track, TrackKind};
use crate::ops::Op;
use crate::time::{Rational, frame_duration};
use crate::tx::{Change, ChangeKind, Diff, EntityKind, Shift};

pub(crate) fn not_found(kind: &str, id: &str) -> CoreError {
    CoreError::new("NOT_FOUND", format!("{kind} {id} does not exist"))
        .with_details(json!({ "id": id, "kind": kind }))
        .with_fix(format!("use the id of an existing {kind}"))
}

/// Raised when a module is handed an op of another kind (a programming error, not user input).
pub(crate) fn wrong_op(op: &Op) -> CoreError {
    CoreError::new("INVALID_OP", format!("{} dispatched to the wrong module", op.name()))
        .with_details(json!({ "op": op.name() }))
}

pub(crate) fn exists(project: &Project, kind: IdKind, id: &str) -> bool {
    match kind {
        IdKind::Asset => project.assets.contains_key(id),
        IdKind::Clip => project.timeline.tracks.iter().any(|t| t.items.iter().any(|c| c.id == id)),
        IdKind::Track => project.timeline.tracks.iter().any(|t| t.id == id),
        IdKind::Timeline => project.timeline.id == id,
    }
}

pub(crate) fn track_index(project: &Project, id: &str) -> Result<usize, CoreError> {
    project.timeline.tracks.iter().position(|t| t.id == id).ok_or_else(|| not_found("track", id))
}

/// `(track index, item index)` of clip `id`.
pub(crate) fn clip_index(project: &Project, id: &str) -> Result<(usize, usize), CoreError> {
    for (ti, track) in project.timeline.tracks.iter().enumerate() {
        if let Some(ci) = track.items.iter().position(|c| c.id == id) {
            return Ok((ti, ci));
        }
    }
    Err(not_found("clip", id))
}

pub(crate) fn asset<'a>(project: &'a Project, id: &str) -> Result<&'a Asset, CoreError> {
    project.assets.get(id).ok_or_else(|| not_found("asset", id))
}

pub(crate) fn end(clip: &Clip) -> Result<Rational, CoreError> {
    clip.start.checked_add(clip.duration)
}

pub(crate) fn invalid_range(message: impl Into<String>, details: serde_json::Value, fix: impl Into<String>) -> CoreError {
    CoreError::new("INVALID_RANGE", message).with_details(details).with_fix(fix)
}

/// The asset must carry the stream the track plays.
pub(crate) fn check_kind(track: &Track, asset: &Asset) -> Result<(), CoreError> {
    let ok = match track.kind {
        TrackKind::Video => asset.video.is_some(),
        TrackKind::Audio => asset.audio.is_some(),
    };
    if ok {
        return Ok(());
    }
    let want = match track.kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    };
    Err(CoreError::new("TRACK_KIND_MISMATCH", format!("asset {} has no {want} stream for track {}", asset.id, track.id))
        .with_details(json!({ "track": track.id, "trackKind": want, "asset": asset.id }))
        .with_fix(format!("put the asset on a track whose kind it has a stream for, not {want}")))
}

/// `source_in >= 0`, `duration >= one frame`, `source_in + duration <= asset.duration`.
pub(crate) fn check_source(
    project: &Project,
    asset: &Asset,
    source_in: Rational,
    duration: Rational,
) -> Result<(), CoreError> {
    let frame = frame_duration(project.rate)?;
    if source_in < Rational::zero() {
        return Err(invalid_range(
            "sourceIn must not be negative",
            json!({ "sourceIn": source_in, "min": Rational::zero() }),
            "use a sourceIn of 0 or more",
        ));
    }
    if duration < frame {
        return Err(invalid_range(
            "duration must be at least one frame",
            json!({ "duration": duration, "min": frame }),
            format!("use a duration of at least {frame}s"),
        ));
    }
    if source_in.checked_add(duration)? > asset.duration {
        let max = asset.duration.checked_sub(source_in)?;
        return Err(invalid_range(
            format!("sourceIn + duration runs past the end of asset {}", asset.id),
            json!({ "sourceIn": source_in, "duration": duration, "assetDuration": asset.duration, "max": max }),
            format!("use a duration of at most {max}s from this sourceIn"),
        ));
    }
    Ok(())
}

/// The clip on `track` strictly containing `at` (`start < at < end`), if any.
pub(crate) fn clip_containing(track: &Track, at: Rational) -> Result<Option<&Clip>, CoreError> {
    for clip in &track.items {
        if clip.start < at && at < end(clip)? {
            return Ok(Some(clip));
        }
    }
    Ok(None)
}

pub(crate) fn insert_inside(clip: &Clip, at: Rational) -> Result<CoreError, CoreError> {
    let (start, end) = (clip.start, end(clip)?);
    let nearest = if at.checked_sub(start)? <= end.checked_sub(at)? { start } else { end };
    Ok(CoreError::new("INSERT_INSIDE_CLIP", format!("{at} is inside clip {}", clip.id))
        .with_details(json!({ "clip": clip.id, "at": at, "start": start, "end": end, "nearest": nearest }))
        .with_fix(format!("insert at the clip's start ({start}) or end ({end}); nearest is {nearest}")))
}

/// Ripple: moves every item on `track` starting at or after `from` by `by`.
pub(crate) fn ripple(track: &mut Track, from: Rational, by: Rational) -> Result<(), CoreError> {
    if by.is_zero() {
        return Ok(());
    }
    for clip in track.items.iter_mut().filter(|c| c.start >= from) {
        clip.start = clip.start.checked_add(by)?;
    }
    Ok(())
}

/// Inserts `clip` keeping `track.items` sorted by `start` (after any item with the same start).
pub(crate) fn place(track: &mut Track, clip: Clip) {
    let i = track.items.partition_point(|c| c.start <= clip.start);
    track.items.insert(i, clip);
}

pub(crate) fn record(diff: &mut Diff, kind: EntityKind, id: &str, change: ChangeKind) {
    diff.changes.push(Change { kind, id: id.to_string(), change });
}

pub(crate) fn shift(diff: &mut Diff, track: &str, from: Rational, by: Rational) {
    diff.shifts.push(Shift { track: track.to_string(), from, by });
}
