//! `item.delete`: takes an item of any kind off its track, and every member of its link group
//! off theirs (plan 4C Decision 19), in member order; `ripple` closes each gap (later items
//! move by `−duration`), `lift` leaves it. The inverse restores them, the last member first.

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{OpDiff, members, wrong_op};
use crate::ops::{Bindings, Op, item_remove};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::ItemDelete(d) = op else {
        return Err(wrong_op(op));
    };
    let mut inverse = Vec::new();
    for member in members(project, &bindings.resolve(&d.item)?)? {
        inverse.push(item_remove::remove(
            project, &member, d.mode, bindings, diff,
        )?);
    }
    Ok(inverse.into_iter().rev().flatten().collect())
}

/// Removes the item; its group's members come from the diff.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ItemDelete(d) = op {
        f.touches.push((d.item.clone(), Touch::Exists));
        f.removes.push(d.item.clone());
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

/// One restore per item it removed.
pub(crate) fn inverse_len(_op: &Op, own: &Diff) -> usize {
    own.changes
        .iter()
        .filter(|c| c.kind == EntityKind::Item && c.change == ChangeKind::Removed)
        .count()
}

/// The wire name.
pub(crate) const NAME: &str = "item.delete";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ItemDelete(d) => vec![&mut d.item],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ItemDelete(d) => vec![&d.item],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
