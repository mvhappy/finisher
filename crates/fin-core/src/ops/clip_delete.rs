//! `clip.delete`: removes a clip and closes the gap (later clips move by `-duration`).

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{clip_index, wrong_op};
use crate::ops::{Bindings, Op, clip_remove};
use crate::tx::Diff;

pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<String, CoreError> {
    let Op::ClipDelete { clip } = op else { return Err(wrong_op(op)) };
    let id = bindings.resolve(clip)?;
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
    clip_remove::remove(project, &id, true, diff)
}
