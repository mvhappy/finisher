//! `track.add`: a new empty track on a timeline (plan 4C Task 12). `id` is a `$name`
//! placeholder bound to a new `t<n>` id; `index` places it in the compositing order (bottom
//! first; the top when absent; past the top is `INVALID_RANGE`, structural); `name` defaults to
//! the kind's letter and its count of that kind's tracks on the timeline (`V2`). A name longer
//! than 64 characters is policy. The diff records the track (`added`); the inverse is
//! `track.remove`. No shift.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{ParamValue, Project, Track, TrackKind};
use crate::ops::track_remove::TrackRemove;
use crate::ops::util::settings::{Scope, check};
use crate::ops::util::{OpDiff, invalid_range, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `track.add`: a new empty track of `kind` on `timeline`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackAdd {
    pub id: String,
    pub timeline: String,
    pub kind: TrackKind,
    // Its place in the timeline's compositing order (0 is the bottom); the top when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub name: Option<String>,
}

impl From<TrackAdd> for Op {
    fn from(op: TrackAdd) -> Op {
        Op::TrackAdd(op)
    }
}

/// The index of timeline `id` (`NOT_FOUND` "timeline").
pub(crate) fn timeline_index(project: &Project, id: &str) -> Result<usize, CoreError> {
    project
        .timelines
        .iter()
        .position(|tl| tl.id == id)
        .ok_or_else(|| crate::ops::util::not_found("timeline", id))
}

/// `INVALID_RANGE` unless `index <= max`.
pub(crate) fn check_index(index: u32, max: usize, what: &str) -> Result<usize, CoreError> {
    let i = usize::try_from(index).unwrap_or(usize::MAX);
    if i > max {
        return Err(invalid_range(
            format!("{what} index {index} is past {max}"),
            json!({ "path": "index", "value": index, "min": 0, "max": max }),
            format!("use an index from 0 to {max}"),
        ));
    }
    Ok(i)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TrackAdd(a) = op else {
        return Err(wrong_op(op));
    };
    let tl = timeline_index(project, &bindings.resolve(&a.timeline)?)?;
    let tracks = &project.timelines[tl].tracks;
    let index = match a.index {
        Some(i) => check_index(i, tracks.len(), "track")?,
        None => tracks.len(),
    };
    let name = match &a.name {
        Some(n) => {
            let mut v = Some(ParamValue::Text(n.clone()));
            check(Scope::Track, "name", v.as_mut(), bindings.policy())?;
            n.clone()
        }
        None => {
            let same = tracks.iter().filter(|t| t.kind == a.kind).count() + 1;
            format!("{}{same}", a.kind.letter())
        }
    };
    let id = bindings.create(project, &a.id, IdKind::Track, ids)?;
    project.timelines[tl]
        .tracks
        .insert(index, Track::new(id.clone(), a.kind, name));
    diff.record(EntityKind::Track, &id, ChangeKind::Added);
    Ok(vec![
        TrackRemove {
            track: id,
            with_items: false,
        }
        .into(),
    ])
}

/// Creates the track on its timeline.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::TrackAdd(a) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&a.id), op_diff);
    f.creates.push(a.id.clone());
    f.uses.push(a.timeline.clone());
    f.uses.push(crate::ops::util::lists::TRACKS.into());
    // An explicit index holds only in the order it was chosen in.
    if a.index.is_some() {
        f.puts_back(crate::ops::util::lists::TRACKS);
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

/// The inverse is one `track.remove`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "track.add";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the new id and the timeline.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TrackAdd(a) => vec![&mut a.id, &mut a.timeline],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TrackAdd(a) => vec![&a.id, &a.timeline],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::TrackAdd(a) => Some(a.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
