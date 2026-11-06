//! `clip.remove` (internal): the inverse of `clip.insert`. `clip.delete` shares `remove`.

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{OpDiff, clip_index, end, ripple, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Removes clip `id`; closes the gap when `rippling`. Returns the `clip.restore` inverse.
pub(crate) fn remove(
    project: &mut Project,
    id: &str,
    rippling: bool,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let (ti, ci) = clip_index(project, id)?;
    let track = ti.get_mut(project);
    let clip = track.items.remove(ci);
    let from = end(&clip)?;
    let by = if rippling {
        clip.duration.checked_neg()?
    } else {
        Rational::zero()
    };
    ripple(track, from, by)?;
    diff.record(EntityKind::Clip, &clip.id, ChangeKind::Removed);
    diff.shift(&track.id, from, by);
    let Some(clip) = clip.to_clip() else {
        return Err(crate::ops::util::not_found("clip", id));
    };
    Ok(vec![Op::ClipRestore {
        clip: Box::new(clip),
        track: track.id.clone(),
        ripple: rippling,
    }])
}

pub(crate) fn validate(
    project: &Project,
    op: &Op,
    bindings: &Bindings,
) -> Result<String, CoreError> {
    let Op::ClipRemove { id, .. } = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(id)?;
    clip_index(project, &id)?;
    Ok(id)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let id = validate(project, op, bindings)?;
    let Op::ClipRemove { ripple, .. } = op else {
        return Err(wrong_op(op));
    };
    remove(project, &id, *ripple, diff)
}

/// Removes the clip, on the track its shift names.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ClipRemove { id, .. } = op {
        f.touches.push((id.clone(), Touch::Exists));
        f.removes.push(id.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "clip.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ClipRemove { id, .. } => vec![id],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ClipRemove { id, .. } => vec![id],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
