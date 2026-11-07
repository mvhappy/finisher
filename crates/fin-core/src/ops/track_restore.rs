//! `track.restore` (internal): the inverse of `track.remove`. Puts a whole track back on
//! `timeline` at `index`, with its items and transitions. Every id it brings back is literal and
//! new (`DUPLICATE_ID`); its items' bodies are checked like an `item.restore`'s, and its items
//! and transitions must form a valid track (structural). Link groups are not checked: their
//! other members kept their `link`. The diff records the track, then its items and transitions
//! (`added`); the inverse is `track.remove` (with its items when it has any). No shift.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Project, Track};
use crate::ops::track_add::{check_index, timeline_index};
use crate::ops::track_remove::TrackRemove;
use crate::ops::util::contents::{
    put_back_track, record_track, returning_track, track_ids, track_refs_mut,
};
use crate::ops::util::{OpDiff, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::{Rational, frame_duration};
use crate::tx::{ChangeKind, Diff};

/// `track.restore` (internal): puts `track` back on `timeline` at `index`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackRestore {
    pub track: Track,
    pub timeline: String,
    pub index: u32,
}

impl From<TrackRestore> for Op {
    fn from(op: TrackRestore) -> Op {
        Op::TrackRestore(Box::new(op))
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TrackRestore(r) = op else {
        return Err(wrong_op(op));
    };
    let tl = timeline_index(project, &bindings.resolve(&r.timeline)?)?;
    let index = check_index(r.index, project.timelines[tl].tracks.len(), "track")?;
    let frame = frame_duration(project.timelines[tl].rate)?;
    let track = returning_track(project, &r.track, frame, bindings, &mut BTreeSet::new())?;
    record_track(&track, ChangeKind::Added, diff);
    let inverse = TrackRemove {
        track: track.id.clone(),
        with_items: !track.items.is_empty(),
    };
    project.timelines[tl].tracks.insert(index, track);
    Ok(vec![inverse.into()])
}

/// Creates the track (its items and transitions come from the diff), on its timeline, using
/// the assets its items show.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::TrackRestore(r) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&r.track.id), op_diff);
    f.creates.push(r.track.id.clone());
    // The track, its items (with their effects) and transitions, as they were.
    put_back_track(&mut f, &r.track);
    f.uses.push(r.timeline.clone());
    for a in r.track.items.iter().filter_map(|i| i.asset()) {
        if !f.uses.iter().any(|u| u == a) {
            f.uses.push(a.to_string());
        }
    }
    f
}

/// No timeline positions: the track comes back whole.
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
pub(crate) const NAME: &str = "track.restore";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably (plan 4C Decision 24): the track's and every id
/// nested in it (items with what they name, their effects and links; transitions with the
/// items they sit between), then the timeline.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TrackRestore(r) => {
            let TrackRestore {
                track, timeline, ..
            } = &mut **r;
            let mut out = track_refs_mut(track);
            out.push(timeline);
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TrackRestore(r) => {
            let mut out = track_ids(&r.track);
            out.push(&r.timeline);
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
