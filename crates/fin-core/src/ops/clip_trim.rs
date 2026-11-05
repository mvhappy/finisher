//! `clip.trim`: moves a clip's start or end edge by `delta`, rippling later clips on the track.
//! Start: `sourceIn += delta`, `duration -= delta`, `start` unchanged, later clips move by
//! `-delta`. End: `duration += delta`, later clips move by `+delta`.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{asset, clip_index, end, invalid_range, record, ripple, shift, wrong_op};
use crate::ops::{Bindings, Edge, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::{Rational, frame_duration};
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Resolved, checked trim: where the clip is.
pub(crate) struct Plan {
    track: usize,
    item: usize,
}

pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<Plan, CoreError> {
    let Op::ClipTrim { clip, edge, delta } = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(clip)?;
    let (ti, ci) = clip_index(project, &id)?;
    let c = &project.timeline.tracks[ti].items[ci];
    let a = asset(project, &c.asset)?;
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
        Edge::Start => (c.source_in.checked_neg()?, c.duration.checked_sub(frame)?),
        Edge::End => (
            frame.checked_sub(c.duration)?,
            a.duration
                .checked_sub(c.source_in)?
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
    diff: &mut Diff,
) -> Result<Vec<Op>, CoreError> {
    let plan = validate(project, op, bindings)?;
    let Op::ClipTrim { edge, delta, .. } = op else {
        return Err(wrong_op(op));
    };
    let track = &mut project.timeline.tracks[plan.track];
    let c = &mut track.items[plan.item];
    let from = end(c)?;
    let by = match edge {
        Edge::Start => {
            c.source_in = c.source_in.checked_add(*delta)?;
            c.duration = c.duration.checked_sub(*delta)?;
            delta.checked_neg()?
        }
        Edge::End => {
            c.duration = c.duration.checked_add(*delta)?;
            *delta
        }
    };
    let id = c.id.clone();
    ripple(track, from, by)?;
    record(diff, EntityKind::Clip, &id, ChangeKind::Updated);
    shift(diff, &track.id, from, by);
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
