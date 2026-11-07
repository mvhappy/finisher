//! `timeline.set`: sets one setting of a timeline (`name`, `resolution`, `rate`; plan 4C Task
//! 12), or resets it with no `value` (only the resolution can be reset, to 1080×1920). An
//! unknown path and a value of the wrong shape are structural; a range is policy, and so is
//! `TIMELINE_NOT_EMPTY`: the rate changes only while no track holds an item. The diff records
//! the timeline's parameter; the inverse sets the old value.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ParamValue, Project};
use crate::ops::track_add::timeline_index;
use crate::ops::util::settings::{Scope, check, set_timeline};
use crate::ops::util::{OpDiff, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// `timeline.set`: sets `path` of `timeline` to `value`; no `value` resets it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelineSet {
    pub timeline: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ParamValue")]
    pub value: Option<ParamValue>,
}

impl From<TimelineSet> for Op {
    fn from(op: TimelineSet) -> Op {
        Op::TimelineSet(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TimelineSet(s) = op else {
        return Err(wrong_op(op));
    };
    let tl = timeline_index(project, &bindings.resolve(&s.timeline)?)?;
    let mut value = s.value.clone();
    check(Scope::Timeline, &s.path, value.as_mut(), bindings.policy())?;
    let timeline = &mut project.timelines[tl];
    if s.path == "rate" && bindings.policy() {
        let items: Vec<&str> = timeline
            .tracks
            .iter()
            .flat_map(|t| t.items.iter().map(|i| i.id.as_str()))
            .collect();
        if !items.is_empty() {
            return Err(CoreError::new(
                "TIMELINE_NOT_EMPTY",
                format!(
                    "timeline {} holds items; its rate cannot change",
                    timeline.id
                ),
            )
            .with_details(json!({ "timeline": timeline.id, "items": items }))
            .with_fix("change the rate before adding items, or use a new timeline"));
        }
    }
    let old = set_timeline(timeline, &s.path, value)?;
    diff.record_path_of(EntityKind::Timeline, &timeline.id, None, &s.path);
    Ok(vec![
        TimelineSet {
            timeline: timeline.id.clone(),
            path: s.path.clone(),
            value: old,
        }
        .into(),
    ])
}

/// Sets the timeline's parameter `path`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::TimelineSet(s) = op {
        f.touches
            .push((s.timeline.clone(), Touch::Param(s.path.clone())));
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

/// The inverse is one `timeline.set`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "timeline.set";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TimelineSet(s) => vec![&mut s.timeline],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TimelineSet(s) => vec![&s.timeline],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
