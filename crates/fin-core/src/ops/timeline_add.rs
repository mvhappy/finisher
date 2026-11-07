//! `timeline.add`: a new empty timeline, after the others (plan 4C Task 12). `id` is a `$name`
//! placeholder bound to a new `tl<n>` id. It has no tracks (one id per op, Decision 17:
//! `track.add` adds them). A positive `rate` and a resolution of at least 1×1 are structural;
//! the name's length, the rate (1 to 240 fps) and the resolution (16 to 16384 px) are policy.
//! The diff records the timeline (`added`); the inverse is `timeline.remove`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{ParamValue, Project, Resolution, Timeline};
use crate::ops::timeline_remove::TimelineRemove;
use crate::ops::util::settings::{Scope, check};
use crate::ops::util::{OpDiff, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `timeline.add`: a new empty timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelineAdd {
    pub id: String,
    pub name: String,
    // Frames per second.
    pub rate: Rational,
    pub resolution: Resolution,
}

impl From<TimelineAdd> for Op {
    fn from(op: TimelineAdd) -> Op {
        Op::TimelineAdd(op)
    }
}

/// Checks a timeline's name, rate and resolution through the timeline settings.
pub(crate) fn check_settings(
    name: &str,
    rate: Rational,
    resolution: Resolution,
    policy: bool,
) -> Result<(), CoreError> {
    let size = [f64::from(resolution.width), f64::from(resolution.height)];
    for (path, mut v) in [
        ("name", ParamValue::Text(name.to_string())),
        ("rate", ParamValue::Text(rate.to_string())),
        ("resolution", ParamValue::Vec2(size)),
    ] {
        check(Scope::Timeline, path, Some(&mut v), policy)?;
    }
    Ok(())
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TimelineAdd(a) = op else {
        return Err(wrong_op(op));
    };
    check_settings(&a.name, a.rate, a.resolution, bindings.policy())?;
    let id = bindings.create(project, &a.id, IdKind::Timeline, ids)?;
    project.timelines.push(Timeline {
        id: id.clone(),
        name: a.name.clone(),
        rate: a.rate,
        resolution: a.resolution,
        markers: Vec::new(),
        tracks: Vec::new(),
    });
    diff.record(EntityKind::Timeline, &id, ChangeKind::Added);
    Ok(vec![TimelineRemove { timeline: id }.into()])
}

/// Creates the timeline.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::TimelineAdd(a) = op {
        f.creates.push(a.id.clone());
        f.uses.push(crate::ops::util::lists::TIMELINES.into());
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

/// The inverse is one `timeline.remove`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "timeline.add";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the new id.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TimelineAdd(a) => vec![&mut a.id],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TimelineAdd(a) => vec![&a.id],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::TimelineAdd(a) => Some(a.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
