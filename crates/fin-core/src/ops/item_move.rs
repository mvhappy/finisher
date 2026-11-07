//! `item.move`: moves an item to a track and position, and its link group with it (plan 4C
//! Decisions 15 and 19): every member moves by the named item's time delta on its own track;
//! only the named item changes track (to one on the same timeline, whose kind fits it).
//!
//! `at` is in the frame before the op. Member by member (the named item first):
//! - `ripple`: a close where it was (`{end, −duration}`), then an open at its target mapped
//!   through the shifts recorded so far, its own close included (`{at', +duration}`). A target
//!   strictly inside the item itself on its own track is `MOVE_INTO_ITSELF` (structural); a
//!   target inside another item is `INSERT_INSIDE_CLIP` (policy).
//! - `overwrite`: every member is lifted first, then each target range is carved
//!   ([`crate::ops::util::carve`]) and the member placed there. No shifts.
//!
//! Two members' legs that meet on one track are `LINK_LEGS_OVERLAP` (structural): any two in
//! `ripple` mode, overlapping targets in `overwrite` mode.
//!
//! The diff records each member as `updated` with `path: "start"`, on the track it left and
//! then on the one it is on when they differ (after any items its carve changed). Inverse
//! (Decision 23, the shifts mirrored in reverse): per member, last member first, `item.remove`
//! from its target, then (ripple) `item.restore` where it was, or (overwrite) the carve's
//! inverses, with the lift restores of every member, last first, at the end. A restore puts
//! the member back whole, so an undo after another author's edit of it (a trim, a split, a
//! move, a parameter) is `SAME_EDGE`/`SAME_PARAM`, never a silent loss
//! (`rebase::footprint`, spec §4.3).

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Item, Project};
use crate::ops::item_insert::check_body;
use crate::ops::item_slide::START;
use crate::ops::util::{
    OpDiff, TrackAt, carve, check_unlocked, clip_containing, frame_of, insert_inside,
    invalid_range, item_index, members, place, ripple, track_index, wrong_op,
};
use crate::ops::{Bindings, DeleteMode, InsertMode, ItemMove, ItemRemove, ItemRestore, Op};
use crate::rebase::{Assoc, Footprint, StepMap, Touch};
use crate::time::Rational;
use crate::tx::{Diff, Shift};

fn payload(op: &Op) -> Result<&ItemMove, CoreError> {
    match op {
        Op::ItemMove(m) => Ok(m),
        _ => Err(wrong_op(op)),
    }
}

/// One member's move: its id, the track it goes to, and its target in the frame before the op.
struct Leg {
    id: String,
    to: TrackAt,
    at: Rational,
    duration: Rational,
}

/// `LINK_LEGS_OVERLAP` (structural) when two members' moves meet on one track: in `ripple`
/// mode any two legs to one track (each opens room there, so the other's target would move
/// by an amount that depends on their order), in `overwrite` mode two target ranges that
/// overlap (the second carve would cut the member the first placed).
fn check_legs(project: &Project, legs: &[Leg], mode: InsertMode) -> Result<(), CoreError> {
    for (i, a) in legs.iter().enumerate() {
        for b in &legs[i + 1..] {
            if a.to != b.to {
                continue;
            }
            let meet = match mode {
                InsertMode::Ripple => true,
                InsertMode::Overwrite => {
                    a.at < b.at.checked_add(b.duration)? && b.at < a.at.checked_add(a.duration)?
                }
            };
            if !meet {
                continue;
            }
            let track = &a.to.get(project).id;
            return Err(CoreError::new(
                "LINK_LEGS_OVERLAP",
                format!(
                    "linked items {} and {} would both move onto track {track}",
                    a.id, b.id
                ),
            )
            .with_details(json!({ "items": [a.id, b.id], "track": track }))
            .with_fix(
                "move the item to a track where its link group's other members do not land, \
                 or unlink the group (item.unlink) and move the items one by one",
            ));
        }
    }
    Ok(())
}

/// The members' legs, checked: the named item fits the target track on its timeline; policy:
/// the locks and non-negative targets.
fn plan(project: &Project, m: &ItemMove, bindings: &Bindings) -> Result<Vec<Leg>, CoreError> {
    let id = bindings.resolve(&m.item)?;
    let to = track_index(project, &bindings.resolve(&m.track)?)?;
    let (from, ci) = item_index(project, &id)?;
    if from.timeline != to.timeline {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("item {id} cannot move to a track of another timeline"),
        )
        .with_details(json!({ "item": id, "track": to.get(project).id }))
        .with_fix("move it to a track of its own timeline"));
    }
    let named = &from.get(project).items[ci];
    check_body(
        project,
        to.get(project),
        &named.body,
        named.duration,
        frame_of(project, to)?,
        bindings,
    )?;
    let delta = m.at.checked_sub(named.start)?;
    let mut legs = Vec::new();
    for member in members(project, &id)? {
        let (t, i) = item_index(project, &member)?;
        let item = &t.get(project).items[i];
        let leg = Leg {
            to: if member == id { to } else { t },
            at: item.start.checked_add(delta)?,
            duration: item.duration,
            id: member,
        };
        if bindings.policy() {
            check_unlocked(t.get(project), Some(&leg.id), bindings)?;
            check_unlocked(leg.to.get(project), Some(&leg.id), bindings)?;
            if leg.at.is_negative() {
                return Err(invalid_range(
                    format!("moving item {} would start it before 0", leg.id),
                    json!({ "item": leg.id, "at": leg.at, "min": "0" }),
                    "move it to 0 or later",
                ));
            }
        }
        legs.push(leg);
    }
    check_legs(project, &legs, m.mode)?;
    Ok(legs)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let m = payload(op)?;
    let legs = plan(project, m, bindings)?;
    match m.mode {
        InsertMode::Ripple => ripple_move(project, legs, bindings, diff),
        InsertMode::Overwrite => overwrite_move(project, legs, diff),
    }
}

/// Records member `id` on the track it left (when it changed track) and the one it is on.
fn record(project: &Project, diff: &mut OpDiff<'_>, id: &str, from: TrackAt, to: TrackAt) {
    if from != to {
        diff.record_path(&from.get(project).id, id, START);
    }
    diff.record_path(&to.get(project).id, id, START);
}

fn ripple_move(
    project: &mut Project,
    legs: Vec<Leg>,
    bindings: &Bindings,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let mut own: Vec<Shift> = Vec::new();
    let mut parts: Vec<[Op; 2]> = Vec::new();
    for leg in legs {
        let (from, ci) = item_index(project, &leg.id)?;
        let item: Item = from.get(project).items[ci].clone();
        let (start, stop) = (item.start, item.end()?);
        let to_id = leg.to.get(project).id.clone();
        let mut at = StepMap::from_shifts(own.clone())
            .map(&to_id, leg.at, Assoc::After)?
            .pos;
        if from == leg.to {
            if start < at && at < stop {
                return Err(CoreError::new(
                    "MOVE_INTO_ITSELF",
                    format!("{at} lies inside item {} itself", leg.id),
                )
                .with_details(json!({ "item": leg.id, "at": at, "start": start, "end": stop }))
                .with_fix(format!(
                    "move it to {start} or {stop} or beyond, or with mode overwrite"
                )));
            }
            if at >= stop {
                at = at.checked_sub(item.duration)?;
            }
        }
        let src = from.get_mut(project);
        src.items.remove(ci);
        let close = item.duration.checked_neg()?;
        ripple(src, stop, close)?;
        let src_id = src.id.clone();
        diff.shift(&src_id, stop, close);
        own.push(Shift {
            track: src_id.clone(),
            from: stop,
            by: close,
            op: 0,
        });
        let dst = leg.to.get_mut(project);
        if bindings.policy()
            && let Some(other) = clip_containing(dst, at)?
        {
            return Err(insert_inside(other, at)?);
        }
        ripple(dst, at, item.duration)?;
        diff.shift(&to_id, at, item.duration);
        own.push(Shift {
            track: to_id,
            from: at,
            by: item.duration,
            op: 0,
        });
        let mut placed = item.clone();
        placed.start = at;
        place(dst, placed);
        record(project, diff, &leg.id, from, leg.to);
        parts.push([
            ItemRemove {
                id: leg.id,
                mode: DeleteMode::Ripple,
            }
            .into(),
            ItemRestore {
                item,
                track: src_id,
                mode: DeleteMode::Ripple,
            }
            .into(),
        ]);
    }
    Ok(parts.into_iter().rev().flatten().collect())
}

fn overwrite_move(
    project: &mut Project,
    legs: Vec<Leg>,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let mut lifted = Vec::new();
    for leg in legs {
        let (from, ci) = item_index(project, &leg.id)?;
        let item = from.get_mut(project).items.remove(ci);
        lifted.push((leg, from, item));
    }
    let mut parts: Vec<Vec<Op>> = Vec::new();
    let mut restores: Vec<Op> = Vec::new();
    for (leg, from, item) in lifted {
        let dst = leg.to.get_mut(project);
        let mut part: Vec<Op> = vec![
            ItemRemove {
                id: leg.id.clone(),
                mode: DeleteMode::Lift,
            }
            .into(),
        ];
        part.extend(carve(
            dst,
            leg.at,
            leg.at.checked_add(item.duration)?,
            diff,
        )?);
        let mut placed = item.clone();
        placed.start = leg.at;
        place(dst, placed);
        record(project, diff, &leg.id, from, leg.to);
        parts.push(part);
        restores.push(
            ItemRestore {
                item,
                track: from.get(project).id.clone(),
                mode: DeleteMode::Lift,
            }
            .into(),
        );
    }
    let mut inverse: Vec<Op> = parts.into_iter().rev().flatten().collect();
    inverse.extend(restores.into_iter().rev());
    Ok(inverse)
}

/// Sets the item's `start` on the target track, which it uses; the members, both tracks of a
/// move between tracks, and what an overwrite carves come from the diff (Decision 20).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::ItemMove(m) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&m.track), op_diff);
    f.touches
        .push((m.item.clone(), Touch::Param(START.to_string())));
    f.uses.push(m.track.clone());
    f
}

/// `at`, on the target track.
pub(crate) fn positions_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::ItemMove(m) => vec![(m.track.clone(), &mut m.at)],
        _ => Vec::new(),
    }
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// Two ops per member (a remove and a restore), and one per change an overwrite's carve
/// recorded (the changes without a `path`).
pub(crate) fn inverse_len(_op: &Op, own: &Diff) -> usize {
    let mut moved: Vec<&str> = own
        .changes
        .iter()
        .filter(|c| c.path.as_deref() == Some(START))
        .map(|c| c.id.as_str())
        .collect();
    moved.dedup();
    let carved = own.changes.iter().filter(|c| c.path.is_none()).count();
    2 * moved.len() + carved
}

/// The wire name.
pub(crate) const NAME: &str = "item.move";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemMove(m) => vec![&mut m.item, &mut m.track],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemMove(m) => vec![&m.item, &m.track],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
