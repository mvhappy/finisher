//! `marker.set`: sets one field of a marker (`label`, `color`, `at`, `duration`; plan 4C Task
//! 13), or clears it with no `value` (a colour or duration; a label or position cannot be
//! reset). An unknown path, a value of the wrong shape and a negative time are structural; a
//! label's length is policy. Setting `at` moves the marker among its timeline's markers (they
//! stay ordered by `at`, then id). The diff records the marker's parameter; the inverse sets
//! the old value.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ParamValue, Project};
use crate::ops::marker_add::{marker_index, sort_markers};
use crate::ops::util::settings::{Scope, check, set_marker};
use crate::ops::util::{OpDiff, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// `marker.set`: sets `path` of `marker` to `value`; no `value` clears it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarkerSet {
    pub marker: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ParamValue")]
    pub value: Option<ParamValue>,
}

impl From<MarkerSet> for Op {
    fn from(op: MarkerSet) -> Op {
        Op::MarkerSet(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::MarkerSet(s) = op else {
        return Err(wrong_op(op));
    };
    let (tl, i) = marker_index(project, &bindings.resolve(&s.marker)?)?;
    let mut value = s.value.clone();
    check(Scope::Marker, &s.path, value.as_mut(), bindings.policy())?;
    let timeline = &mut project.timelines[tl];
    let id = timeline.markers[i].id.clone();
    let old = set_marker(&mut timeline.markers[i], &s.path, value)?;
    sort_markers(timeline);
    diff.record_path_of(EntityKind::Marker, &id, None, &s.path);
    Ok(vec![
        MarkerSet {
            marker: id,
            path: s.path.clone(),
            value: old,
        }
        .into(),
    ])
}

/// Sets the marker's parameter `path`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::MarkerSet(s) = op {
        f.touches
            .push((s.marker.clone(), Touch::Param(s.path.clone())));
    }
    f
}

/// No timeline positions to map: a marker's `at` never ripples (Decision 26).
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The inverse is one `marker.set`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "marker.set";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::MarkerSet(s) => vec![&mut s.marker],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::MarkerSet(s) => vec![&s.marker],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
