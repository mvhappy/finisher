//! `transition.remove`: takes a transition off its track. Policy: the lock. The diff records
//! it (`removed`, on its track); the inverse is the internal `transition.set` restoring it.

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::transition_set::restore;
use crate::ops::util::{OpDiff, check_unlocked, find_transition, not_found, wrong_op};
use crate::ops::{Bindings, Op};
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
    let Op::TransitionRemove(r) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&r.transition)?;
    let ti = find_transition(project, &id).ok_or_else(|| not_found("transition", &id))?;
    check_unlocked(ti.get(project), None, bindings)?;
    let track = ti.get_mut(project);
    let i = track
        .transitions
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| not_found("transition", &id))?;
    let t = track.transitions.remove(i);
    diff.record_on(&track.id, EntityKind::Transition, &id, ChangeKind::Removed);
    Ok(vec![restore(t, &track.id).into()])
}

/// Removes the transition.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::TransitionRemove(r) = op {
        f.touches.push((r.transition.clone(), Touch::Exists));
        f.removes.push(r.transition.clone());
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

/// The inverse is one restore.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "transition.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TransitionRemove(r) => vec![&mut r.transition],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TransitionRemove(r) => vec![&r.transition],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
