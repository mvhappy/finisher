//! `clip.insert`: places a clip at a boundary (or in a gap / past the end) and ripples later
//! clips on the track by its duration.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Clip, Project};
use crate::ops::util::{
    asset, check_kind, check_source, clip_containing, insert_inside, invalid_range, place, record,
    ripple, shift, track_index, wrong_op,
};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, LegacyConflict, inserted_range, shift_position};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Resolved, checked insert.
pub(crate) struct Plan {
    track: usize,
    asset: String,
}

pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<Plan, CoreError> {
    let Op::ClipInsert {
        id,
        track,
        at,
        asset: asset_ref,
        source_in,
        duration,
    } = op
    else {
        return Err(wrong_op(op));
    };
    let ti = track_index(project, &bindings.resolve(track)?)?;
    let asset_id = bindings.resolve(asset_ref)?;
    let a = asset(project, &asset_id)?;
    let track = &project.timeline.tracks[ti];
    check_kind(track, a)?;
    // Policy (skipped on replay). An insert inside a clip also overlaps it, which replay still
    // rejects through the structural track check.
    if bindings.policy() {
        if *at < Rational::zero() {
            return Err(invalid_range(
                "insert position must not be negative",
                json!({ "at": at, "min": Rational::zero() }),
                "insert at 0 or later",
            ));
        }
        check_source(project, a, *source_in, *duration)?;
        if let Some(clip) = clip_containing(track, *at)? {
            return Err(insert_inside(clip, *at)?);
        }
    }
    bindings.check_new(project, id, IdKind::Clip)?;
    Ok(Plan {
        track: ti,
        asset: asset_id,
    })
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut Diff,
) -> Result<Vec<Op>, CoreError> {
    let plan = validate(project, op, bindings)?;
    let Op::ClipInsert {
        id,
        at,
        source_in,
        duration,
        ..
    } = op
    else {
        return Err(wrong_op(op));
    };
    let id = bindings.create(project, id, IdKind::Clip, ids)?;
    let track = &mut project.timeline.tracks[plan.track];
    ripple(track, *at, *duration)?;
    let clip = Clip {
        id: id.clone(),
        asset: plan.asset,
        start: *at,
        source_in: *source_in,
        duration: *duration,
    };
    place(track, clip);
    record(diff, EntityKind::Clip, &id, ChangeKind::Added);
    shift(diff, &track.id, *at, *duration);
    Ok(vec![Op::ClipRemove { id, ripple: true }])
}

/// Moves `at` with the committed op's shift; conflicts when it lands strictly inside a clip the
/// committed op placed.
pub(crate) fn rebase(op: &Op, over: &Op, over_diff: &Diff) -> Result<Op, LegacyConflict> {
    let Op::ClipInsert { track, at, .. } = op else {
        return Ok(op.clone());
    };
    let moved = shift_position(track, *at, over_diff)?;
    if let Some((t, start, end)) = inserted_range(over, over_diff)?
        && &t == track
        && start < moved
        && moved < end
    {
        return Err(LegacyConflict::new("insert lands inside a clip"));
    }
    let mut out = op.clone();
    if let Op::ClipInsert { at, .. } = &mut out {
        *at = moved;
    }
    Ok(out)
}

/// Creates the clip on its track, using the track and the asset.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::ClipInsert {
        id, track, asset, ..
    } = op
    else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(track), op_diff);
    f.creates.push(id.clone());
    f.uses.extend([asset.clone(), track.clone()]);
    f
}

/// `at`, on the insert's track.
pub(crate) fn positions_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::ClipInsert { track, at, .. } => vec![(track.clone(), at)],
        _ => Vec::new(),
    }
}
