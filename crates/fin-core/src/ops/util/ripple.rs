//! Ripple, placement and diff-recording helpers.

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Clip, Track};
use crate::ops::util::end;
use crate::time::Rational;
use crate::tx::{Change, ChangeKind, Diff, EntityKind, Shift};

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
    let nearest = if at.checked_sub(start)? <= end.checked_sub(at)? {
        start
    } else {
        end
    };
    Ok(CoreError::new(
        "INSERT_INSIDE_CLIP",
        format!("{at} is inside clip {}", clip.id),
    )
    .with_details(
        json!({ "clip": clip.id, "at": at, "start": start, "end": end, "nearest": nearest }),
    )
    .with_fix(format!(
        "insert at the clip's start ({start}) or end ({end}); nearest is {nearest}"
    )))
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
    diff.changes.push(Change {
        kind,
        id: id.to_string(),
        change,
    });
}

pub(crate) fn shift(diff: &mut Diff, track: &str, from: Rational, by: Rational) {
    diff.shifts.push(Shift {
        track: track.to_string(),
        from,
        by,
    });
}
