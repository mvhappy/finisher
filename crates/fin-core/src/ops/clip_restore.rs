//! `clip.restore` (internal): the inverse of `clip.delete`/`clip.remove`. Puts the clip back at
//! its original `start`, rippling later clips by its duration when `ripple`. Its clip id may be
//! a `$placeholder` (spec §4.3): the clip then gets a newly allocated id, which replay hands out
//! again from the recorded bindings.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::Project;
use crate::ops::util::TrackAt;
use crate::ops::util::{
    OpDiff, asset, check_kind, check_source, clip_containing, end, insert_inside, invalid_range,
    place, ripple, track_index, wrong_op,
};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

pub(crate) fn validate(
    project: &Project,
    op: &Op,
    bindings: &Bindings,
) -> Result<TrackAt, CoreError> {
    let Op::ClipRestore {
        clip,
        track,
        ripple,
    } = op
    else {
        return Err(wrong_op(op));
    };
    bindings.check_new(project, &clip.id, IdKind::Clip)?;
    let ti = track_index(project, &bindings.resolve(track)?)?;
    let a = asset(project, &bindings.resolve(&clip.asset)?)?;
    let track = ti.get(project);
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
        let stop = clip.start.checked_add(clip.duration)?;
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
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
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
    clip.id = bindings.create(project, &clip.id, IdKind::Clip, ids)?;
    clip.asset = bindings.resolve(&clip.asset)?;
    let track = ti.get_mut(project);
    let by = if *rippling {
        clip.duration
    } else {
        Rational::zero()
    };
    ripple(track, clip.start, by)?;
    diff.record(EntityKind::Clip, &clip.id, ChangeKind::Added);
    diff.shift(&track.id, clip.start, by);
    let id = clip.id.clone();
    place(track, (*clip).into());
    Ok(vec![Op::ClipRemove {
        id,
        ripple: *rippling,
    }])
}

/// Creates the clip on its track, using the track and the asset.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::ClipRestore { clip, track, .. } = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(track), op_diff);
    f.creates.push(clip.id.clone());
    f.uses.extend([clip.asset.clone(), track.clone()]);
    f
}

/// The clip's `start`, on the restore's track.
pub(crate) fn positions_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::ClipRestore { clip, track, .. } => vec![(track.clone(), &mut clip.start)],
        _ => Vec::new(),
    }
}

/// The wire name.
pub(crate) const NAME: &str = "clip.restore";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ClipRestore { clip, track, .. } => vec![&mut clip.id, &mut clip.asset, track],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ClipRestore { clip, track, .. } => vec![&clip.id, &clip.asset, track],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::ClipRestore { clip, .. } => Some(clip.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
