//! `item.remove` (internal): the inverse of `item.insert`. `item.delete` shares [`remove`].

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{OpDiff, check_unlocked, end, item_index, ripple, wrong_op};
use crate::ops::{Bindings, DeleteMode, ItemRestore, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Takes item `id` off its track; `ripple` closes the gap (recording the shift), `lift` leaves
/// it (and records none). Returns the `item.restore` inverse.
pub(crate) fn remove(
    project: &mut Project,
    id: &str,
    mode: DeleteMode,
    bindings: &Bindings,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let (ti, ci) = item_index(project, id)?;
    check_unlocked(ti.get(project), Some(id), bindings)?;
    let track = ti.get_mut(project);
    let item = track.items.remove(ci);
    diff.record(EntityKind::Item, &item.id, ChangeKind::Removed);
    if mode == DeleteMode::Ripple {
        let from = end(&item)?;
        let by = item.duration.checked_neg()?;
        ripple(track, from, by)?;
        diff.shift(&track.id, from, by);
    }
    Ok(vec![
        ItemRestore {
            item,
            track: track.id.clone(),
            mode,
        }
        .into(),
    ])
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::ItemRemove(r) = op else {
        return Err(wrong_op(op));
    };
    remove(project, &bindings.resolve(&r.id)?, r.mode, bindings, diff)
}

/// Removes the item (on the track its shift names, when it ripples).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemRemove(r) = op {
        f.touches.push((r.id.clone(), Touch::Exists));
        f.removes.push(r.id.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "item.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemRemove(r) => vec![&mut r.id],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemRemove(r) => vec![&r.id],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
