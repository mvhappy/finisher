//! `item.slide`: moves an item (alone, plan 4C Decision 19) by `delta` between its neighbours.
//! The neighbour touching its start (the one before) moves its end by `delta`; the one touching
//! its end (the one after) moves its start by `delta` (with its source and content, recording
//! `by`). Nothing else moves and it never ripples. A side with no touching neighbour needs room
//! in the gap the item moves into (`SLIDE_BLOCKED`, structural). The item keeps its source and
//! its item-relative times. Policy: the lock, `start ≥ 0`, and each neighbour's edge limits
//! (one frame, its source).
//!
//! The inverse names the neighbours it trimmed (internal `sides`), so an undo trims exactly
//! those back even when the slide left the item touching another one.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Project, Track};
use crate::ops::util::{
    OpDiff, TrackAt, check_delta, check_track, check_unlocked, edge_limits, end, invalid_range,
    item_index, move_start_edge, wrong_op,
};
use crate::ops::{Bindings, Edge, ItemSlide, Op, SlideSides};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

/// The parameter a slide (and a move) sets (plan 4C Decision 20).
pub(crate) const START: &str = "start";

fn payload(op: &Op) -> Result<&ItemSlide, CoreError> {
    match op {
        Op::ItemSlide(s) => Ok(s),
        _ => Err(wrong_op(op)),
    }
}

fn not_adjacent(track: &Track, item: &str, other: &str) -> CoreError {
    CoreError::new(
        "NOT_ADJACENT",
        format!("item {other} does not touch item {item} to slide against"),
    )
    .with_details(json!({ "item": item, "neighbour": other, "track": track.id }))
}

/// The indexes of the neighbours trimmed on each side: named (`sides`, each must touch) or
/// found by touch.
fn sides(
    track: &Track,
    ci: usize,
    sides: Option<&SlideSides>,
    bindings: &Bindings,
) -> Result<(Option<usize>, Option<usize>), CoreError> {
    let it = &track.items[ci];
    let (start, stop) = (it.start, end(it)?);
    let before = ci
        .checked_sub(1)
        .filter(|&i| end(&track.items[i]).is_ok_and(|e| e == start));
    let after = Some(ci + 1).filter(|&i| track.items.get(i).is_some_and(|n| n.start == stop));
    let Some(named) = sides else {
        return Ok((before, after));
    };
    let pick = |name: &Option<String>, found: Option<usize>| -> Result<Option<usize>, CoreError> {
        let Some(name) = name else { return Ok(None) };
        let name = bindings.resolve(name)?;
        match found {
            Some(i) if track.items[i].id == name => Ok(Some(i)),
            _ => Err(not_adjacent(track, &it.id, &name)),
        }
    };
    Ok((pick(&named.before, before)?, pick(&named.after, after)?))
}

fn blocked(track: &Track, item: &str, delta: Rational) -> CoreError {
    CoreError::new(
        "SLIDE_BLOCKED",
        format!(
            "item {item} has no room to slide by {delta} on track {}",
            track.id
        ),
    )
    .with_details(json!({ "item": item, "delta": delta, "track": track.id }))
    .with_fix("slide by less, or against a touching neighbour")
}

/// Checks the slide: the room on a side with no neighbour (structural), then policy.
fn validate(
    project: &Project,
    ti: TrackAt,
    ci: usize,
    delta: Rational,
    (before, after): (Option<usize>, Option<usize>),
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let track = ti.get(project);
    let it = &track.items[ci];
    let (start, stop) = (it.start, end(it)?);
    if delta.is_positive()
        && after.is_none()
        && let Some(next) = track.items.get(ci + 1)
        && next.start < stop.checked_add(delta)?
    {
        return Err(blocked(track, &it.id, delta));
    }
    if delta.is_negative()
        && before.is_none()
        && let Some(prev) = ci.checked_sub(1).map(|i| &track.items[i])
        && end(prev)? > start.checked_add(delta)?
    {
        return Err(blocked(track, &it.id, delta));
    }
    if !bindings.policy() {
        return Ok(());
    }
    check_unlocked(track, Some(&it.id), bindings)?;
    if start.checked_add(delta)?.is_negative() {
        return Err(invalid_range(
            format!("sliding item {} by {delta} moves it before 0", it.id),
            json!({ "item": it.id, "delta": delta, "min": start.checked_neg()? }),
            format!("slide by {} or more", start.checked_neg()?),
        ));
    }
    if let Some(b) = before {
        let id = &track.items[b].id;
        check_delta(
            id,
            Edge::End,
            delta,
            edge_limits(project, ti, b, Edge::End)?,
        )?;
    }
    if let Some(a) = after {
        let id = &track.items[a].id;
        check_delta(
            id,
            Edge::Start,
            delta,
            edge_limits(project, ti, a, Edge::Start)?,
        )?;
    }
    Ok(())
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let s = payload(op)?;
    if s.sides.is_some() && !bindings.internal() {
        return Err(CoreError::new("INVALID_OP", "a slide's sides are internal")
            .with_details(json!({ "op": "item.slide", "item": s.item }))
            .with_fix("leave sides out: a slide trims the neighbours touching the item"));
    }
    let id = bindings.resolve(&s.item)?;
    let (ti, ci) = item_index(project, &id)?;
    let found = sides(ti.get(project), ci, s.sides.as_ref(), bindings)?;
    validate(project, ti, ci, s.delta, found, bindings)?;
    let track = ti.get_mut(project);
    let it = &mut track.items[ci];
    it.start = it.start.checked_add(s.delta)?;
    let mut sides = SlideSides {
        before: None,
        after: None,
    };
    diff.record_path(&track.id, &id, START);
    if let Some(b) = found.0 {
        let n = &mut track.items[b];
        n.duration = n.duration.checked_add(s.delta)?;
        sides.before = Some(n.id.clone());
        diff.record_edge(&track.id, &n.id, Edge::End, None);
    }
    if let Some(a) = found.1 {
        let n = &mut track.items[a];
        let by = move_start_edge(n, s.delta)?;
        n.start = n.start.checked_add(s.delta)?;
        sides.after = Some(n.id.clone());
        diff.record_edge(&track.id, &n.id, Edge::Start, Some(by));
    }
    check_track(track)?;
    Ok(vec![
        ItemSlide {
            item: id,
            delta: s.delta.checked_neg()?,
            sides: Some(sides),
        }
        .into(),
    ])
}

/// Sets the item's `start`; its neighbours' edges come from the diff (Decision 20).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemSlide(s) = op {
        f.touches
            .push((s.item.clone(), Touch::Param(START.to_string())));
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

/// The inverse is one slide back.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "item.slide";
/// Whether the op is produced only as an inverse and never accepted from clients (`sides`
/// alone is internal).
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item, then the named sides.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemSlide(s) => {
            let ItemSlide { item, sides, .. } = s;
            let mut out = vec![item];
            if let Some(SlideSides { before, after }) = sides {
                out.extend(before.as_mut());
                out.extend(after.as_mut());
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemSlide(s) => {
            let mut out = vec![s.item.as_str()];
            if let Some(sides) = &s.sides {
                out.extend(sides.before.as_deref());
                out.extend(sides.after.as_deref());
            }
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
