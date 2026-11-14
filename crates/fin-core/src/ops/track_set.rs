//! `track.set`: sets one setting of a track (`name`, `mute`, `solo`, `lock`, `gain`; plan 4C
//! Task 12), or resets it with no `value` (a name cannot be reset). An unknown path and a value
//! of the wrong shape are structural (`PARAM_UNKNOWN`, `PARAM_TYPE`); a range is policy. A
//! locked track's settings stay editable (the lock guards its items). The diff records the
//! track's parameter; the inverse sets the old value. No shift.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ParamValue, Project};
use crate::ops::util::settings::{Scope, check, set_track};
use crate::ops::util::{OpDiff, track_index, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// `track.set`: sets `path` of `track` to `value`; no `value` resets it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackSet {
    pub track: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ParamValue")]
    pub value: Option<ParamValue>,
}

impl From<TrackSet> for Op {
    fn from(op: TrackSet) -> Op {
        Op::TrackSet(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TrackSet(s) = op else {
        return Err(wrong_op(op));
    };
    let at = track_index(project, &bindings.resolve(&s.track)?)?;
    let mut value = s.value.clone();
    check(Scope::Track, &s.path, value.as_mut(), bindings.policy())?;
    let track = at.get_mut(project);
    let old = set_track(track, &s.path, value)?;
    diff.record_path_of(EntityKind::Track, &track.id, None, &s.path);
    Ok(vec![
        TrackSet {
            track: track.id.clone(),
            path: s.path.clone(),
            value: old,
        }
        .into(),
    ])
}

/// Sets the track's parameter `path`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::TrackSet(s) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&s.track), op_diff);
    f.touches
        .push((s.track.clone(), Touch::Param(s.path.clone())));
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

/// The inverse is one `track.set`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "track.set";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TrackSet(s) => vec![&mut s.track],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TrackSet(s) => vec![&s.track],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
