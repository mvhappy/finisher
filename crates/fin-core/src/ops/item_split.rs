//! `item.split`: cuts an item in two at `offset` (item-relative, plan 4C Decision 13).
//!
//! The left part keeps the id and ends at `offset`. The right part copies the body and the
//! common fields (effects keep their ids; keyframes move by `−offset`; a clip's or nested
//! item's source moves by `offset`, a clip's times its speed) and gets no link group (a split
//! of a linked item splits that item only, Decision 17). Its id is the op's one allocation.
//!
//! `offset` lies strictly inside the item (structural: neither part may be empty) and at least
//! one frame from each end (policy). A caption is `INVALID_OP` (`caption.split` splits its
//! words). The inverse is internal: `item.remove` of the right part (lift), then a `strict`
//! trim of the left part's end by the right part's length, so an undo meets a later edit of the
//! right part's range as a conflict (B4).

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{ItemBody, Project};
use crate::ops::util::{
    OpDiff, TrackAt, check_unlocked, frame_of, invalid_range, item_index, place, shift_content,
    wrong_op,
};
use crate::ops::{Bindings, DeleteMode, Edge, ItemRemove, ItemSplit, ItemTrim, Op, TrimMode};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff};

fn payload(op: &Op) -> Result<&ItemSplit, CoreError> {
    match op {
        Op::ItemSplit(s) => Ok(s),
        _ => Err(wrong_op(op)),
    }
}

/// Where the item is, once checked.
fn validate(
    project: &Project,
    s: &ItemSplit,
    id: &str,
    bindings: &Bindings,
) -> Result<(TrackAt, usize), CoreError> {
    let (ti, ci) = item_index(project, id)?;
    let track = ti.get(project);
    let it = &track.items[ci];
    if let ItemBody::Caption { .. } = it.body {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("item {id} is a caption: item.split does not split its words"),
        )
        .with_details(json!({ "item": id, "kind": "caption" }))
        .with_fix("use caption.split"));
    }
    if !s.offset.is_positive() || s.offset >= it.duration {
        return Err(invalid_range(
            format!("offset {} is not strictly inside item {id}", s.offset),
            json!({ "item": id, "offset": s.offset, "min": "0", "max": it.duration,
                    "exclusive": true }),
            format!(
                "split at an offset between 0 and {} (exclusive)",
                it.duration
            ),
        ));
    }
    bindings.check_new(project, &s.id, IdKind::of_item(&it.body))?;
    if !bindings.policy() {
        return Ok((ti, ci));
    }
    check_unlocked(track, Some(id), bindings)?;
    let frame = frame_of(project, ti)?;
    let max = it.duration.checked_sub(frame)?;
    if s.offset < frame || s.offset > max {
        return Err(invalid_range(
            format!(
                "splitting item {id} at {} leaves a part under one frame",
                s.offset
            ),
            json!({ "item": id, "offset": s.offset, "min": frame, "max": max }),
            format!("split at an offset between {frame} and {max}"),
        ));
    }
    Ok((ti, ci))
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let s = payload(op)?;
    let id = bindings.resolve(&s.item)?;
    let (ti, ci) = validate(project, s, &id, bindings)?;
    let kind = IdKind::of_item(&ti.get(project).items[ci].body);
    let right_id = bindings.create(project, &s.id, kind, ids)?;
    let track = ti.get_mut(project);
    let left = &mut track.items[ci];
    let mut right = left.clone();
    right.id.clone_from(&right_id);
    right.start = left.start.checked_add(s.offset)?;
    right.duration = left.duration.checked_sub(s.offset)?;
    match &mut right.body {
        ItemBody::Clip {
            source_in, speed, ..
        } => *source_in = source_in.checked_add(s.offset.checked_mul(*speed)?)?,
        ItemBody::Nested { source_in, .. } => *source_in = source_in.checked_add(s.offset)?,
        _ => {}
    }
    shift_content(&mut right, s.offset.checked_neg()?)?;
    right.common.link = None;
    left.duration = s.offset;
    let length = right.duration;
    place(track, right);
    diff.record_item(&track.id, &id, ChangeKind::Updated);
    diff.record_item(&track.id, &right_id, ChangeKind::Added);
    Ok(vec![
        ItemRemove {
            id: right_id,
            mode: DeleteMode::Lift,
        }
        .into(),
        ItemTrim {
            item: id,
            edge: Edge::End,
            delta: length,
            mode: TrimMode::Strict,
        }
        .into(),
    ])
}

/// Needs the item and changes it as a whole; creates the right part (Decision 20).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemSplit(s) = op {
        f.touches.push((s.item.clone(), Touch::Exists));
        f.creates.push(s.id.clone());
    }
    f
}

/// No timeline positions: `offset` counts from the item's start.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// `offset`, relative to the item it splits (Decision 16).
pub(crate) fn offsets_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::ItemSplit(s) => vec![(s.item.clone(), &mut s.offset)],
        _ => Vec::new(),
    }
}

/// The wire name.
pub(crate) const NAME: &str = "item.split";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item, and the right part's new id.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemSplit(s) => vec![&mut s.item, &mut s.id],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemSplit(s) => vec![&s.item, &s.id],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::ItemSplit(s) => Some(s.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
