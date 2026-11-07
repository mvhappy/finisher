//! `timeline.remove`: takes a timeline off the project with everything on it (plan 4C Task
//! 12). Structural: the project keeps one timeline (`INVALID_OP` for the last), and a timeline a
//! `nested` item on another timeline shows cannot go (`TIMELINE_NESTED`, `details.items`).
//! Policy: a locked track on it (`TRACK_LOCKED`). The diff records every removed entity (each
//! track's items and transitions, then the track; the markers; then the timeline); the inverse
//! is the internal `timeline.restore` of the whole timeline at its index. No shift.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ItemBody, Project};
use crate::ops::timeline_restore::TimelineRestore;
use crate::ops::track_add::timeline_index;
use crate::ops::util::contents::record_track;
use crate::ops::util::{OpDiff, check_unlocked, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `timeline.remove`: takes `timeline` off the project with its tracks, items and markers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelineRemove {
    pub timeline: String,
}

impl From<TimelineRemove> for Op {
    fn from(op: TimelineRemove) -> Op {
        Op::TimelineRemove(op)
    }
}

fn check(project: &Project, tl: usize, bindings: &Bindings) -> Result<(), CoreError> {
    let id = &project.timelines[tl].id;
    if project.timelines.len() == 1 {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("timeline {id} is the project's last timeline"),
        )
        .with_details(json!({ "timeline": id }))
        .with_fix("add another timeline first; a project keeps at least one"));
    }
    let nesting: Vec<&str> = project
        .timelines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != tl)
        .flat_map(|(_, other)| other.tracks.iter().flat_map(|t| t.items.iter()))
        .filter(|i| matches!(&i.body, ItemBody::Nested { timeline, .. } if timeline == id))
        .map(|i| i.id.as_str())
        .collect();
    if !nesting.is_empty() {
        return Err(CoreError::new(
            "TIMELINE_NESTED",
            format!("timeline {id} is shown by nested items"),
        )
        .with_details(json!({ "timeline": id, "items": nesting }))
        .with_fix("delete the nested items that show it first"));
    }
    for track in &project.timelines[tl].tracks {
        check_unlocked(track, None, bindings)?;
    }
    Ok(())
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TimelineRemove(r) = op else {
        return Err(wrong_op(op));
    };
    let tl = timeline_index(project, &bindings.resolve(&r.timeline)?)?;
    check(project, tl, bindings)?;
    let timeline = project.timelines.remove(tl);
    for track in &timeline.tracks {
        record_track(track, ChangeKind::Removed, diff);
    }
    for m in &timeline.markers {
        diff.record(EntityKind::Marker, &m.id, ChangeKind::Removed);
    }
    diff.record(EntityKind::Timeline, &timeline.id, ChangeKind::Removed);
    Ok(vec![
        TimelineRestore {
            timeline,
            index: u32::try_from(tl).unwrap_or(u32::MAX),
        }
        .into(),
    ])
}

/// Needs the timeline and removes it; what went with it comes from the diff (Decision 20).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::TimelineRemove(r) = op {
        f.touches.push((r.timeline.clone(), Touch::Exists));
        f.removes.push(r.timeline.clone());
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

/// The inverse is one `timeline.restore`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "timeline.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TimelineRemove(r) => vec![&mut r.timeline],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TimelineRemove(r) => vec![&r.timeline],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
