//! `item.trim`: moves an item's start or end edge by `delta`.
//!
//! - `ripple`, start edge: `start` stays; `duration −= delta`; the source moves under it
//!   (`sourceIn += delta × speed`, plan 4C Decision 14); later items move by `−delta`.
//! - `ripple`, end edge: `duration += delta`; later items move by `+delta`.
//! - `strict` (internal): the edge itself moves (a start edge moves `start` by `delta`
//!   too), nothing else does, and an overlap it makes is `CLIP_OVERLAP` in every apply mode.
//!
//! A start-edge move shifts the item's relative times (keyframes, caption words) by `−delta`
//! and records that content offset as the change's `by` (Decision 13).

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ItemBody, Project};
use crate::ops::util::{
    OpDiff, TrackAt, asset, check_track, check_unlocked, end, frame_of, invalid_range, item_index,
    media_duration, ripple, shift_content, wrong_op,
};
use crate::ops::{Bindings, Edge, ItemTrim, Op, TrimMode};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

fn payload(op: &Op) -> Result<&ItemTrim, CoreError> {
    match op {
        Op::ItemTrim(t) => Ok(t),
        _ => Err(wrong_op(op)),
    }
}

/// Resolved, checked trim: where the item is.
pub(crate) struct Plan {
    track: TrackAt,
    item: usize,
}

pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<Plan, CoreError> {
    let t = payload(op)?;
    if t.mode == TrimMode::Strict && !bindings.internal() {
        return Err(CoreError::new("INVALID_OP", "strict trims are internal")
            .with_details(json!({ "op": "item.trim", "mode": "strict" }))
            .with_fix("trim with mode ripple"));
    }
    let id = bindings.resolve(&t.item)?;
    let (ti, ci) = item_index(project, &id)?;
    let plan = Plan {
        track: ti,
        item: ci,
    };
    // Policy (skipped on replay): the lock, the source range and the one-frame minimum.
    if !bindings.policy() {
        return Ok(plan);
    }
    let track = ti.get(project);
    check_unlocked(track, Some(&id), bindings)?;
    let it = &track.items[ci];
    let frame = frame_of(project, ti)?;
    // The source a trim may reveal: clips are bounded by their asset, nested items by 0.
    let (source_in, speed, length) = match &it.body {
        ItemBody::Clip {
            asset: a,
            source_in,
            speed,
        } => (
            Some(*source_in),
            *speed,
            Some(media_duration(asset(project, a)?)?),
        ),
        ItemBody::Nested { source_in, .. } => (Some(*source_in), Rational::from_int(1), None),
        _ => (None, Rational::from_int(1), None),
    };
    let (mut min, max) = match t.edge {
        Edge::Start => (
            source_in
                .map(|s| s.checked_neg()?.checked_div(speed))
                .transpose()?,
            Some(it.duration.checked_sub(frame)?),
        ),
        Edge::End => (
            Some(frame.checked_sub(it.duration)?),
            match (source_in, length) {
                (Some(s), Some(l)) => Some(
                    l.checked_sub(s)?
                        .checked_div(speed)?
                        .checked_sub(it.duration)?,
                ),
                _ => None,
            },
        ),
    };
    if t.edge == Edge::Start && t.mode == TrimMode::Strict {
        let floor = it.start.checked_neg()?;
        min = Some(min.map_or(floor, |m| m.max(floor)));
    }
    if min.is_some_and(|m| t.delta < m) || max.is_some_and(|m| t.delta > m) {
        let range = |v: Option<Rational>| v.map_or("unbounded".to_string(), |v| v.to_string());
        return Err(invalid_range(
            format!(
                "trimming item {id} by {} would leave its source range or go below one frame",
                t.delta
            ),
            json!({ "clip": id, "item": id, "edge": t.edge, "delta": t.delta, "min": min, "max": max }),
            format!("use a delta between {} and {}", range(min), range(max)),
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
    let t = payload(op)?;
    let delta = t.delta;
    let track = plan.track.get_mut(project);
    let it = &mut track.items[plan.item];
    let from = end(it)?;
    let (by, content) = match t.edge {
        Edge::Start => {
            match &mut it.body {
                ItemBody::Clip {
                    source_in, speed, ..
                } => *source_in = source_in.checked_add(delta.checked_mul(*speed)?)?,
                ItemBody::Nested { source_in, .. } => *source_in = source_in.checked_add(delta)?,
                _ => {}
            }
            it.duration = it.duration.checked_sub(delta)?;
            if t.mode == TrimMode::Strict {
                it.start = it.start.checked_add(delta)?;
            }
            let content = delta.checked_neg()?;
            shift_content(it, content)?;
            (content, Some(content))
        }
        Edge::End => {
            it.duration = it.duration.checked_add(delta)?;
            (delta, None)
        }
    };
    let id = it.id.clone();
    diff.record_edge(EntityKind::Item, &id, t.edge, content);
    match t.mode {
        TrimMode::Ripple => {
            ripple(track, from, by)?;
            diff.shift(&track.id, from, by);
        }
        TrimMode::Strict => check_track(track)?,
    }
    Ok(vec![
        ItemTrim {
            item: id,
            edge: t.edge,
            delta: delta.checked_neg()?,
            mode: t.mode,
        }
        .into(),
    ])
}

/// Moves one edge of the item (on the track its shift names, when it ripples).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemTrim(t) = op {
        f.touches.push((t.item.clone(), Touch::Edge(t.edge)));
    }
    f
}

/// No timeline positions: `delta` is a length.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "item.trim";
/// Whether the op is produced only as an inverse and never accepted from clients (`strict`
/// alone is internal, checked by [`validate`]).
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemTrim(t) => vec![&mut t.item],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemTrim(t) => vec![&t.item],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
