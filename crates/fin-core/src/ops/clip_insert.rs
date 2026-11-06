//! `clip.insert`: places a clip at a boundary (or in a gap / past the end) and ripples later
//! clips on the track by its duration.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Clip, Project};
use crate::ops::util::{
    OpDiff, asset, check_kind, check_source, clip_containing, insert_inside, invalid_range, place,
    ripple, track_index, wrong_op,
};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
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
    diff: &mut OpDiff<'_>,
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
    diff.record(EntityKind::Clip, &id, ChangeKind::Added);
    diff.shift(&track.id, *at, *duration);
    Ok(vec![Op::ClipRemove { id, ripple: true }])
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

/// The wire name.
pub(crate) const NAME: &str = "clip.insert";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ClipInsert {
            id, track, asset, ..
        } => vec![id, track, asset],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ClipInsert {
            id, track, asset, ..
        } => vec![id, track, asset],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::ClipInsert { id, .. } => Some(id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
