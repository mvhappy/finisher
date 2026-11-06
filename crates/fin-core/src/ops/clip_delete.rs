//! `clip.delete`: removes a clip and closes the gap (later clips move by `-duration`).

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{OpDiff, clip_index, wrong_op};
use crate::ops::{Bindings, Op, clip_remove};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

pub(crate) fn validate(
    project: &Project,
    op: &Op,
    bindings: &Bindings,
) -> Result<String, CoreError> {
    let Op::ClipDelete { clip } = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(clip)?;
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
    clip_remove::remove(project, &id, true, diff)
}

/// Removes the clip, on the track its shift names.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::ClipDelete { clip } = op {
        f.touches.push((clip.clone(), Touch::Exists));
        f.removes.push(clip.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "clip.delete";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::ClipDelete { clip } => vec![clip],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::ClipDelete { clip } => vec![clip],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
