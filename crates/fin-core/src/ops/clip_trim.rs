//! `clip.trim`: moves a clip's start or end edge by `delta`, rippling later clips on the track.
//! Start: `sourceIn += delta`, `duration -= delta`, `start` unchanged, later clips move by
//! `-delta`. End: `duration += delta`, later clips move by `+delta`.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::ItemBody;
use crate::model::Project;
use crate::ops::util::{
    OpDiff, TrackAt, asset, clip_index, end, invalid_range, media_duration, ripple, wrong_op,
};
use crate::ops::{Bindings, Edge, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::{Rational, frame_duration};
use crate::tx::{Diff, EntityKind};

/// Resolved, checked trim: where the clip is.
pub(crate) struct Plan {
    track: TrackAt,
    item: usize,
}

pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<Plan, CoreError> {
    let Op::ClipTrim { clip, edge, delta } = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(clip)?;
    let (ti, ci) = clip_index(project, &id)?;
    let c = &ti.get(project).items[ci];
    let ItemBody::Clip {
        asset: a,
        source_in,
        ..
    } = &c.body
    else {
        return Err(crate::ops::util::not_found("clip", &id));
    };
    let (a, source_in) = (asset(project, a)?, *source_in);
    let plan = Plan {
        track: ti,
        item: ci,
    };
    // Policy (skipped on replay): the source range and the one-frame minimum.
    if !bindings.policy() {
        return Ok(plan);
    }
    let frame = frame_duration(project.rate)?;
    let (min, max) = match edge {
        Edge::Start => (source_in.checked_neg()?, c.duration.checked_sub(frame)?),
        Edge::End => (
            frame.checked_sub(c.duration)?,
            media_duration(a)?
                .checked_sub(source_in)?
                .checked_sub(c.duration)?,
        ),
    };
    if *delta < min || *delta > max {
        return Err(invalid_range(
            format!(
                "trimming clip {id} by {delta} would leave the asset's source range or go below one frame"
            ),
            json!({ "clip": id, "edge": edge, "delta": delta, "min": min, "max": max }),
            format!("use a delta between {min} and {max}"),
        ));
    }
    Ok(plan)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let plan = validate(project, op, bindings)?;
    let Op::ClipTrim { edge, delta, .. } = op else {
        return Err(wrong_op(op));
    };
    let track = plan.track.get_mut(project);
    let c = &mut track.items[plan.item];
    let from = end(c)?;
    // `content`: the clip's source moved under its fixed start (Decision 13).
    let (by, content) = match edge {
        Edge::Start => {
            if let ItemBody::Clip { source_in, .. } = &mut c.body {
                *source_in = source_in.checked_add(*delta)?;
            }
            c.duration = c.duration.checked_sub(*delta)?;
            (delta.checked_neg()?, Some(delta.checked_neg()?))
        }
        Edge::End => {
            c.duration = c.duration.checked_add(*delta)?;
            (*delta, None)
        }
    };
    let id = c.id.clone();
    ripple(track, from, by)?;
    diff.record_edge(EntityKind::Clip, &id, *edge, content);
    diff.shift(&track.id, from, by);
    Ok(vec![Op::ClipTrim {
        clip: id,
        edge: *edge,
        delta: delta.checked_neg()?,
    }])
}

/// Moves one edge of the clip, on the track its shift names.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ClipTrim { clip, edge, .. } = op {
        f.touches.push((clip.clone(), Touch::Edge(*edge)));
    }
    f
}

/// No timeline positions: `delta` is a length.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "clip.trim";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ClipTrim { clip, .. } => vec![clip],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ClipTrim { clip, .. } => vec![clip],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
