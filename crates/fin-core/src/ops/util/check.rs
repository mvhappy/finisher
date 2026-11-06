//! Range, kind and structural checks shared by the op modules.

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Asset, Project, Track, TrackKind};
use crate::ops::util::{end, track_index};
use crate::time::{Rational, frame_duration};
use crate::tx::Shift;

pub(crate) fn invalid_range(
    message: impl Into<String>,
    details: serde_json::Value,
    fix: impl Into<String>,
) -> CoreError {
    CoreError::new("INVALID_RANGE", message)
        .with_details(details)
        .with_fix(fix)
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
            format!(
                "sourceIn + duration runs past the end of asset {}",
                asset.id
            ),
            json!({ "sourceIn": source_in, "duration": duration, "assetDuration": asset.duration, "max": max }),
            format!("use a duration of at most {max}s from this sourceIn"),
        ));
    }
    Ok(())
}

/// The structural invariant of a track: positive durations (`CLIP_EMPTY`), non-negative
/// `start` and `sourceIn` (`CLIP_NEGATIVE`), sorted by `start` with no overlaps
/// (`CLIP_OVERLAP`). Policy checks keep it on the edit path; replay, which skips them, checks it
/// after every op on each track the op shifted.
pub(crate) fn check_tracks(project: &Project, shifts: &[Shift]) -> Result<(), CoreError> {
    for s in shifts {
        check_track(&project.timeline.tracks[track_index(project, &s.track)?])?;
    }
    Ok(())
}

fn check_track(track: &Track) -> Result<(), CoreError> {
    for c in &track.items {
        if !c.duration.is_positive() {
            return Err(CoreError::new(
                "CLIP_EMPTY",
                format!("clip {} has no positive duration", c.id),
            )
            .with_details(json!({ "track": track.id, "clip": c.id, "duration": c.duration })));
        }
        if c.start < Rational::zero() || c.source_in < Rational::zero() {
            return Err(CoreError::new(
                "CLIP_NEGATIVE",
                format!("clip {} has a negative start or sourceIn", c.id),
            )
            .with_details(json!({
                "track": track.id,
                "clip": c.id,
                "start": c.start,
                "sourceIn": c.source_in,
            })));
        }
    }
    for w in track.items.windows(2) {
        let prev_end = end(&w[0])?;
        if w[1].start < prev_end {
            return Err(CoreError::new(
                "CLIP_OVERLAP",
                format!(
                    "clips {} and {} overlap on track {}",
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
