//! `item.restore` (internal): the inverse of `item.delete`/`item.remove`. Puts the item back at
//! its `start`; `ripple` moves later items by its duration (recording the shift), `lift` moves
//! nothing (and records none), and an overlap it would make is `CLIP_OVERLAP`, so an undo
//! meets a later edit of the gap as a conflict. Its id may be a
//! `$placeholder` (spec §4.3): the item then gets a newly allocated id, which replay hands
//! out again from the recorded bindings. Policy: the item's effects fit the track's domain
//! (`TRACK_KIND_MISMATCH`, plan 6C Decision 13).

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::Project;
use crate::ops::item_insert::{check_body, resolve_body};
use crate::ops::util::keys::check_key_lists;
use crate::ops::util::{
    OpDiff, TrackAt, check_effect_domains, check_track, check_unlocked, clip_containing, frame_of,
    insert_inside, invalid_range, normalize_item, place, ripple, track_index, wrong_op,
};
use crate::ops::{Bindings, DeleteMode, ItemRemove, ItemRestore, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff};

fn payload(op: &Op) -> Result<&ItemRestore, CoreError> {
    match op {
        Op::ItemRestore(r) => Ok(r),
        _ => Err(wrong_op(op)),
    }
}

pub(crate) fn validate(
    project: &Project,
    op: &Op,
    bindings: &Bindings,
) -> Result<TrackAt, CoreError> {
    let r = payload(op)?;
    let item = &r.item;
    bindings.check_new(project, &item.id, IdKind::of_item(&item.body))?;
    let ti = track_index(project, &bindings.resolve(&r.track)?)?;
    let track = ti.get(project);
    check_body(
        project,
        track,
        &item.body,
        item.duration,
        frame_of(project, ti)?,
        bindings,
    )?;
    // Policy (skipped on replay); overlaps are caught there by the structural track check.
    if !bindings.policy() {
        return Ok(ti);
    }
    check_unlocked(track, Some(&item.id), bindings)?;
    let kinds = item.common.effects.iter().map(|e| e.kind.as_str());
    check_effect_domains(track, item, kinds, bindings)?;
    if item.start < Rational::zero() {
        return Err(invalid_range(
            "item start must not be negative",
            json!({ "start": item.start }),
            "restore at 0 or later",
        ));
    }
    if r.mode == DeleteMode::Ripple
        && let Some(other) = clip_containing(track, item.start)?
    {
        return Err(insert_inside(other, item.start)?);
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
    let r = payload(op)?;
    let mut item = r.item.clone();
    normalize_item(&mut item)?;
    check_key_lists(&item.common.keyframes, [], !bindings.policy())?;
    item.id = bindings.create(project, &item.id, IdKind::of_item(&item.body), ids)?;
    resolve_body(&mut item.body, bindings)?;
    let id = item.id.clone();
    let track = ti.get_mut(project);
    diff.record_item(&track.id, &id, ChangeKind::Added);
    if r.mode == DeleteMode::Ripple {
        ripple(track, item.start, item.duration)?;
        diff.shift(&track.id, item.start, item.duration);
    }
    place(track, item);
    // A lift restore into a gap that is no longer free overlaps (policy here; replay's
    // touched-track check finds it too).
    if r.mode == DeleteMode::Lift && bindings.policy() {
        check_track(track)?;
    }
    Ok(vec![ItemRemove { id, mode: r.mode }.into()])
}

/// Creates the item on its track, using the track and the asset it shows, and puts it back
/// whole (`Footprint::puts_back`): its snapshot sets every edge, parameter and effect, so an
/// intervening edit of the item or its effects (another author's trim, split or effect edit
/// of a moved item, before the move's undo) is a conflict, not lost (spec §4.3).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::ItemRestore(r) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&r.track), op_diff);
    f.creates.push(r.item.id.clone());
    f.puts_back(&r.item.id);
    f.uses.extend(r.item.asset().map(str::to_string));
    f.uses.push(r.track.clone());
    f
}

/// The item's `start`, on the restore's track.
pub(crate) fn positions_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::ItemRestore(r) => {
            let ItemRestore { item, track, .. } = &mut **r;
            vec![(track.clone(), &mut item.start)]
        }
        _ => Vec::new(),
    }
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The op's inverse is one op.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "item.restore";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably (plan 4C Decision 24): the item's own, what its
/// body names, its effects' and link group's ids, and the track.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemRestore(r) => {
            let ItemRestore { item, track, .. } = &mut **r;
            let mut out = item.refs_mut();
            out.push(track);
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemRestore(r) => {
            let mut out = r.item.ids();
            out.push(&r.track);
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::ItemRestore(r) => Some(r.item.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
