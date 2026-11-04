//! `clip.restore` (internal): the inverse of `clip.delete`/`clip.remove`. Puts the clip back at
//! its original `start`, rippling later clips by its duration when `ripple`.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::Project;
use crate::ops::util::{
    asset, check_kind, check_source, clip_containing, end, insert_inside, invalid_range, place,
    record, ripple, shift, track_index, wrong_op,
};
use crate::ops::{Bindings, Op};
use crate::rebase::{Conflict, inserted_range, shift_position};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

pub(crate) fn validate(
    project: &Project,
    op: &Op,
    bindings: &Bindings,
) -> Result<usize, CoreError> {
    let Op::ClipRestore {
        clip,
        track,
        ripple,
    } = op
    else {
        return Err(wrong_op(op));
    };
    // Replay: a restore names its clip literally (it never allocates).
    if !bindings.policy() {
        crate::ops::check_literal(&clip.id, IdKind::Clip)?;
    }
    bindings.check_new(project, &clip.id, IdKind::Clip)?;
    let ti = track_index(project, &bindings.resolve(track)?)?;
    let a = asset(project, &bindings.resolve(&clip.asset)?)?;
    let track = &project.timeline.tracks[ti];
    check_kind(track, a)?;
    // Policy (skipped on replay); overlaps are caught there by the structural track check.
    if !bindings.policy() {
        return Ok(ti);
    }
    check_source(project, a, clip.source_in, clip.duration)?;
    if clip.start < Rational::zero() {
        return Err(invalid_range(
            "clip start must not be negative",
            json!({ "start": clip.start }),
            "restore at 0 or later",
        ));
    }
    if let Some(other) = clip_containing(track, clip.start)? {
        return Err(insert_inside(other, clip.start)?);
    }
    if !*ripple {
        let stop = end(clip)?;
        for other in &track.items {
            if other.start < stop && clip.start < end(other)? {
                return Err(invalid_range(
                    format!(
                        "restoring {} without ripple overlaps clip {}",
                        clip.id, other.id
                    ),
                    json!({ "clip": other.id, "start": clip.start, "end": stop }),
                    "restore with ripple, or clear the range first",
                ));
            }
        }
    }
    Ok(ti)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut Diff,
) -> Result<Vec<Op>, CoreError> {
    let ti = validate(project, op, bindings)?;
    let Op::ClipRestore {
        clip,
        ripple: rippling,
        ..
    } = op
    else {
        return Err(wrong_op(op));
    };
    let mut clip = clip.clone();
    clip.asset = bindings.resolve(&clip.asset)?;
    let track = &mut project.timeline.tracks[ti];
    let by = if *rippling {
        clip.duration
    } else {
        Rational::zero()
    };
    ripple(track, clip.start, by)?;
    record(diff, EntityKind::Clip, &clip.id, ChangeKind::Added);
    shift(diff, &track.id, clip.start, by);
    let id = clip.id.clone();
    place(track, clip);
    Ok(vec![Op::ClipRemove {
        id,
        ripple: *rippling,
    }])
}

/// Moves the clip's `start` like an insert position; conflicts when the committed op restored
/// the same clip or placed a clip around the new start.
pub(crate) fn rebase(op: &Op, over: &Op, over_diff: &Diff) -> Result<Op, Conflict> {
    let Op::ClipRestore { clip, track, .. } = op else {
        return Ok(op.clone());
    };
    if let Op::ClipRestore { clip: other, .. } = over
        && other.id == clip.id
    {
        return Err(Conflict::new("clip already restored"));
    }
    let moved = shift_position(track, clip.start, over_diff)?;
    if let Some((t, start, end)) = inserted_range(over, over_diff)?
        && &t == track
        && start < moved
        && moved < end
    {
        return Err(Conflict::new("restore lands inside a clip"));
    }
    let mut out = op.clone();
    if let Op::ClipRestore { clip, .. } = &mut out {
        clip.start = moved;
    }
    Ok(out)
}
