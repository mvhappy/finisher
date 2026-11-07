//! `item.trim`: moves an item's start or end edge by `delta`.
//!
//! - `ripple`, start edge: `start` stays; `duration −= delta`; the source moves under it
//!   (`sourceIn += delta × speed`, plan 4C Decision 14); later items move by `−delta`.
//! - `ripple`, end edge: `duration += delta`; later items move by `+delta`.
//! - `roll`: the cut between the item and the neighbour touching that edge moves by `delta`
//!   (`NOT_ADJACENT` without one, in every apply mode): the left item's end and the right
//!   item's start (with its source) move, nothing else does. Its inverse is a roll back.
//! - `overwrite`: the edge itself moves (a start edge moves `start` by `delta` too); extending
//!   carves the range it covers ([`carve`]), shrinking leaves a gap. No shifts. Its inverse is a
//!   `strict` trim back, then the carve's inverses.
//! - `strict` (internal): the edge itself moves, nothing else does, and an overlap it makes is
//!   `CLIP_OVERLAP` in every apply mode.
//!
//! A start-edge move shifts that item's relative times (keyframes, caption words) by `−delta`
//! and records that content offset as the change's `by` (Decision 13).
//!
//! Every member of the item's link group is trimmed the same way, in member order (plan 4C
//! Decision 19; a policy refusal of any member refuses the op); the inverse trims each back
//! alone (internal `alone`), the last member first. `strict` trims are always alone.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Project, Track};
use crate::ops::util::roll::{back, neighbour, roll};
use crate::ops::util::{
    Limits, OpDiff, TrackAt, carve, check_delta, check_track, check_unlocked, edge_limits, end,
    intersect, item_index, members, move_start_edge, ripple, wrong_op,
};
use crate::ops::{Bindings, Edge, ItemTrim, Op, TrimMode};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

fn payload(op: &Op) -> Result<&ItemTrim, CoreError> {
    match op {
        Op::ItemTrim(t) => Ok(t),
        _ => Err(wrong_op(op)),
    }
}

/// Resolved, checked trim of one item: where it is, and for a roll, its neighbour across
/// the edge.
struct Plan {
    track: TrackAt,
    item: usize,
    neighbour: Option<usize>,
}

/// The mode and flag checks clients may not pass.
fn check_internal(t: &ItemTrim, bindings: &Bindings) -> Result<(), CoreError> {
    let internal = match (t.mode, t.alone) {
        (TrimMode::Strict, _) => Some(("strict trims are internal", "mode")),
        (_, true) => Some((
            "trims of one item of a link group alone are internal",
            "alone",
        )),
        _ => None,
    };
    match internal {
        Some((message, field)) if !bindings.internal() => {
            Err(CoreError::new("INVALID_OP", message)
                .with_details(json!({ "op": "item.trim", "field": field }))
                .with_fix("trim with mode ripple, roll or overwrite; a link group trims together"))
        }
        _ => Ok(()),
    }
}

fn validate(
    project: &Project,
    t: &ItemTrim,
    id: &str,
    bindings: &Bindings,
) -> Result<Plan, CoreError> {
    let (ti, ci) = item_index(project, id)?;
    let track = ti.get(project);
    let plan = Plan {
        track: ti,
        item: ci,
        neighbour: match t.mode {
            TrimMode::Roll => Some(neighbour(track, ci, t.edge)?),
            _ => None,
        },
    };
    // Policy (skipped on replay): the lock, the source range and the one-frame minimum.
    if !bindings.policy() {
        return Ok(plan);
    }
    check_unlocked(track, Some(id), bindings)?;
    let mut limits: Limits = edge_limits(project, ti, ci, t.edge)?;
    match (t.mode, plan.neighbour) {
        // The start edge moves `start` itself, which stays at 0 or later.
        (TrimMode::Strict | TrimMode::Overwrite, _) if t.edge == Edge::Start => {
            limits = intersect(limits, (Some(track.items[ci].start.checked_neg()?), None));
        }
        // The neighbour's opposite edge moves by the same delta.
        (TrimMode::Roll, Some(n)) => {
            let other = match t.edge {
                Edge::Start => Edge::End,
                Edge::End => Edge::Start,
            };
            limits = intersect(limits, edge_limits(project, ti, n, other)?);
        }
        _ => {}
    }
    check_delta(id, t.edge, t.delta, limits)?;
    Ok(plan)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let t = payload(op)?;
    check_internal(t, bindings)?;
    let id = bindings.resolve(&t.item)?;
    let group = if t.alone || t.mode == TrimMode::Strict {
        vec![id]
    } else {
        members(project, &id)?
    };
    let mut inverse = Vec::new();
    for member in group {
        inverse.push(trim_one(project, t, &member, bindings, diff)?);
    }
    Ok(inverse.into_iter().rev().flatten().collect())
}

/// Trims item `id` alone; returns its inverse.
fn trim_one(
    project: &mut Project,
    t: &ItemTrim,
    id: &str,
    bindings: &Bindings,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let plan = validate(project, t, id, bindings)?;
    let track = plan.track.get_mut(project);
    match (t.mode, plan.neighbour) {
        (TrimMode::Roll, Some(n)) => roll(track, plan.item, n, t, diff),
        (TrimMode::Ripple, _) => {
            let (id, from, by) = move_edge(track, plan.item, t, false, diff)?;
            ripple(track, from, by)?;
            diff.shift(&track.id, from, by);
            Ok(vec![back(&id, t, TrimMode::Ripple)?])
        }
        (TrimMode::Overwrite, _) => {
            let it = &track.items[plan.item];
            let (start, stop) = (it.start, end(it)?);
            let range = match t.edge {
                Edge::End => (stop, stop.checked_add(t.delta)?),
                Edge::Start => (start.checked_add(t.delta)?, start),
            };
            let id = it.id.clone();
            let carved = carve(track, range.0, range.1, diff)?;
            // Carving may have removed items before it.
            let index = track
                .items
                .iter()
                .position(|c| c.id == id)
                .unwrap_or(plan.item);
            move_edge(track, index, t, true, diff)?;
            let mut inverse = vec![back(&id, t, TrimMode::Strict)?];
            inverse.extend(carved);
            Ok(inverse)
        }
        _ => {
            let (id, _, _) = move_edge(track, plan.item, t, true, diff)?;
            check_track(track)?;
            Ok(vec![back(&id, t, TrimMode::Strict)?])
        }
    }
}

/// Moves `t.edge` of item `index` by `t.delta` and records it; `moves_start`: a start edge
/// moves `start` too (every mode but ripple). Returns the item's id, its old end and the
/// length change.
fn move_edge(
    track: &mut Track,
    index: usize,
    t: &ItemTrim,
    moves_start: bool,
    diff: &mut OpDiff<'_>,
) -> Result<(String, Rational, Rational), CoreError> {
    let it = &mut track.items[index];
    let from = end(it)?;
    let (by, content) = match t.edge {
        Edge::Start => {
            let content = move_start_edge(it, t.delta)?;
            if moves_start {
                it.start = it.start.checked_add(t.delta)?;
            }
            (content, Some(content))
        }
        Edge::End => {
            it.duration = it.duration.checked_add(t.delta)?;
            (t.delta, None)
        }
    };
    diff.record_edge(&track.id, &it.id, t.edge, content);
    Ok((it.id.clone(), from, by))
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

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// Per member, every mode inverts to one trim back (an item's edge change), and an overwrite
/// trim also to one op per item it carved (each records one change).
pub(crate) fn inverse_len(op: &Op, own: &Diff) -> usize {
    let changes = own.changes.len();
    match op {
        // A roll records the item and its neighbour, and inverts to one roll back.
        Op::ItemTrim(t) if t.mode == TrimMode::Roll => changes / 2,
        _ => changes,
    }
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
