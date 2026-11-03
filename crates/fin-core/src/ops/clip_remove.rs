//! `clip.remove` (internal): the inverse of `clip.insert`. `clip.delete` shares `remove`.

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{clip_index, end, record, ripple, shift, wrong_op};
use crate::ops::{Bindings, Op};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Removes clip `id`; closes the gap when `rippling`. Returns the `clip.restore` inverse.
pub(crate) fn remove(project: &mut Project, id: &str, rippling: bool, diff: &mut Diff) -> Result<Vec<Op>, CoreError> {
    let (ti, ci) = clip_index(project, id)?;
    let track = &mut project.timeline.tracks[ti];
    let clip = track.items.remove(ci);
    let from = end(&clip)?;
    let by = if rippling { clip.duration.checked_neg()? } else { Rational::zero() };
    ripple(track, from, by)?;
    record(diff, EntityKind::Clip, &clip.id, ChangeKind::Removed);
    shift(diff, &track.id, from, by);
    Ok(vec![Op::ClipRestore { clip, track: track.id.clone(), ripple: rippling }])
}

pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<String, CoreError> {
    let Op::ClipRemove { id, .. } = op else { return Err(wrong_op(op)) };
    let id = bindings.resolve(id)?;
    clip_index(project, &id)?;
    Ok(id)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut Diff,
) -> Result<Vec<Op>, CoreError> {
    let id = validate(project, op, bindings)?;
    let Op::ClipRemove { ripple, .. } = op else { return Err(wrong_op(op)) };
    remove(project, &id, *ripple, diff)
}
