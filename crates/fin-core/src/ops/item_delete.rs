//! `item.delete`: takes an item of any kind off its track; `ripple` closes the gap (later
//! items move by `−duration`), `lift` leaves it.

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{OpDiff, wrong_op};
use crate::ops::{Bindings, Op, item_remove};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

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
    item_remove::remove(project, &bindings.resolve(&d.item)?, d.mode, bindings, diff)
}

/// Removes the item (on the track its shift names, when it ripples).
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
