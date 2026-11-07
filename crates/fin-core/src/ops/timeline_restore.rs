//! `timeline.restore` (internal): the inverse of `timeline.remove`. Puts a whole timeline back
//! at `index`, with its markers, tracks, items and transitions. Every id it brings back is
//! literal and new (`DUPLICATE_ID`), each track is checked like a `track.restore`'s, and its
//! settings like a `timeline.add`'s. The diff records the timeline, its markers, then each
//! track with its items and transitions (`added`); the inverse is `timeline.remove`.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Project, Timeline};
use crate::ops::timeline_add::check_settings;
use crate::ops::timeline_remove::TimelineRemove;
use crate::ops::track_add::check_index;
use crate::ops::util::contents::{
    check_returning, record_track, returning_track, track_ids, track_refs_mut,
};
use crate::ops::util::{OpDiff, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::{Rational, frame_duration};
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `timeline.restore` (internal): puts `timeline` back at `index` of the project's timelines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimelineRestore {
    pub timeline: Timeline,
    pub index: u32,
}

impl From<TimelineRestore> for Op {
    fn from(op: TimelineRestore) -> Op {
        Op::TimelineRestore(Box::new(op))
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TimelineRestore(r) = op else {
        return Err(wrong_op(op));
    };
    let index = check_index(r.index, project.timelines.len(), "timeline")?;
    let src = &r.timeline;
    check_settings(&src.name, src.rate, src.resolution, bindings.policy())?;
    let frame = frame_duration(src.rate)?;
    let mut seen = BTreeSet::new();
    check_returning(project, &src.id, IdKind::Timeline, bindings, &mut seen)?;
    for m in &src.markers {
        check_returning(project, &m.id, IdKind::Marker, bindings, &mut seen)?;
    }
    let tracks = src
        .tracks
        .iter()
        .map(|t| returning_track(project, t, frame, bindings, &mut seen))
        .collect::<Result<Vec<_>, _>>()?;
    diff.record(EntityKind::Timeline, &src.id, ChangeKind::Added);
    for m in &src.markers {
        diff.record(EntityKind::Marker, &m.id, ChangeKind::Added);
    }
    for t in &tracks {
        record_track(t, ChangeKind::Added, diff);
    }
    let timeline = Timeline {
        tracks,
        ..src.clone()
    };
    project.timelines.insert(index, timeline);
    Ok(vec![
        TimelineRemove {
            timeline: src.id.clone(),
        }
        .into(),
    ])
}

/// Creates the timeline (what is on it comes from the diff), using the assets its items show.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::TimelineRestore(r) = op {
        f.creates.push(r.timeline.id.clone());
        let items = r.timeline.tracks.iter().flat_map(|t| t.items.iter());
        for a in items.filter_map(|i| i.asset()) {
            if !f.uses.iter().any(|u| u == a) {
                f.uses.push(a.to_string());
            }
        }
    }
    f
}

/// No timeline positions: the timeline comes back whole.
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
pub(crate) const NAME: &str = "timeline.restore";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably (plan 4C Decision 24): the timeline's, its
/// markers', and every id nested in its tracks.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    let Op::TimelineRestore(r) = op else {
        return Vec::new();
    };
    let Timeline {
        id,
        markers,
        tracks,
        ..
    } = &mut r.timeline;
    let mut out = vec![id];
    out.extend(markers.iter_mut().map(|m| &mut m.id));
    for t in tracks {
        out.extend(track_refs_mut(t));
    }
    out
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    let Op::TimelineRestore(r) = op else {
        return Vec::new();
    };
    let tl = &r.timeline;
    let mut out = vec![tl.id.as_str()];
    out.extend(tl.markers.iter().map(|m| m.id.as_str()));
    for t in &tl.tracks {
        out.extend(track_ids(t));
    }
    out
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
