//! `marker.remove`: takes a marker off its timeline (plan 4C Task 13). The diff records it
//! (`removed`); the inverse is `marker.add` with its literal id and every field.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::marker_add::{MarkerAdd, marker_index};
use crate::ops::util::{OpDiff, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `marker.remove`: takes `marker` off its timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarkerRemove {
    pub marker: String,
}

impl From<MarkerRemove> for Op {
    fn from(op: MarkerRemove) -> Op {
        Op::MarkerRemove(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::MarkerRemove(r) = op else {
        return Err(wrong_op(op));
    };
    let (tl, i) = marker_index(project, &bindings.resolve(&r.marker)?)?;
    let timeline = &mut project.timelines[tl];
    let m = timeline.markers.remove(i);
    diff.record(EntityKind::Marker, &m.id, ChangeKind::Removed);
    Ok(vec![
        MarkerAdd {
            id: m.id,
            timeline: timeline.id.clone(),
            at: m.at,
            duration: m.duration,
            label: m.label,
            color: m.color,
        }
        .into(),
    ])
}

/// Needs the marker and removes it.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::MarkerRemove(r) = op {
        f.touches.push((r.marker.clone(), Touch::Exists));
        f.removes.push(r.marker.clone());
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

/// The inverse is one `marker.add`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "marker.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::MarkerRemove(r) => vec![&mut r.marker],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::MarkerRemove(r) => vec![&r.marker],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
