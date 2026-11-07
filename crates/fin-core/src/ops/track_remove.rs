//! `track.remove`: takes a track off its timeline (plan 4C Task 12). A track holding items is
//! `TRACK_NOT_EMPTY` (structural) unless `withItems`: then its items and transitions go with
//! it, and the other members of their link groups keep their `link`. A locked track is
//! `TRACK_LOCKED` (policy). The diff records every removed entity (items and transitions on
//! the track, then the track); the inverse is the internal `track.restore` of the whole track
//! at its index. No shift, and the transition pass does not run (the track is gone).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::track_restore::TrackRestore;
use crate::ops::util::contents::record_track;
use crate::ops::util::{OpDiff, check_unlocked, track_index, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff};

/// `track.remove`: takes `track` off its timeline, with its items when `withItems`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackRemove {
    pub track: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub with_items: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl From<TrackRemove> for Op {
    fn from(op: TrackRemove) -> Op {
        Op::TrackRemove(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TrackRemove(r) = op else {
        return Err(wrong_op(op));
    };
    let at = track_index(project, &bindings.resolve(&r.track)?)?;
    let track = at.get(project);
    if !track.items.is_empty() && !r.with_items {
        return Err(CoreError::new(
            "TRACK_NOT_EMPTY",
            format!("track {} holds {} items", track.id, track.items.len()),
        )
        .with_details(json!({
            "track": track.id,
            "items": track.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        }))
        .with_fix("remove it with its items (--with-items), or delete them first"));
    }
    check_unlocked(track, None, bindings)?;
    let timeline = &mut project.timelines[at.timeline];
    let track = timeline.tracks.remove(at.track);
    record_track(&track, ChangeKind::Removed, diff);
    diff.on_timeline(&track.id, &timeline.id);
    Ok(vec![
        TrackRestore {
            track,
            timeline: timeline.id.clone(),
            index: u32::try_from(at.track).unwrap_or(u32::MAX),
        }
        .into(),
    ])
}

/// Needs the track and removes it; what went with it comes from the diff (Decision 20).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::TrackRemove(r) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&r.track), op_diff);
    f.touches.push((r.track.clone(), Touch::Exists));
    f.removes.push(r.track.clone());
    f.uses
        .extend(crate::ops::util::lists::tracks_of(op_diff, &r.track));
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

/// The inverse is one `track.restore`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "track.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TrackRemove(r) => vec![&mut r.track],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TrackRemove(r) => vec![&r.track],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
