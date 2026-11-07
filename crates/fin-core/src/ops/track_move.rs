//! `track.move`: moves a track to another place in its timeline's compositing order (plan 4C
//! Task 12). `index` counts in the order after the move (0 is the bottom; past the top is
//! `INVALID_RANGE`, structural). The diff records the track's `index` parameter; the inverse
//! moves it back. No shift.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::track_add::check_index;
use crate::ops::util::{OpDiff, track_index, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// The parameter a track move sets.
pub(crate) const INDEX: &str = "index";

/// `track.move`: puts `track` at `index` of its timeline's tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackMove {
    pub track: String,
    pub index: u32,
}

impl From<TrackMove> for Op {
    fn from(op: TrackMove) -> Op {
        Op::TrackMove(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::TrackMove(m) = op else {
        return Err(wrong_op(op));
    };
    let at = track_index(project, &bindings.resolve(&m.track)?)?;
    let tracks = &mut project.timelines[at.timeline].tracks;
    let to = check_index(m.index, tracks.len() - 1, "track")?;
    let track = tracks.remove(at.track);
    let id = track.id.clone();
    tracks.insert(to, track);
    diff.record_path_of(EntityKind::Track, &id, None, INDEX);
    diff.on_timeline(&id, &project.timelines[at.timeline].id);
    Ok(vec![
        TrackMove {
            track: id,
            index: u32::try_from(at.track).unwrap_or(u32::MAX),
        }
        .into(),
    ])
}

/// Sets the track's `index`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::TrackMove(m) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&m.track), op_diff);
    f.touches
        .push((m.track.clone(), Touch::Param(INDEX.into())));
    let order = crate::ops::util::lists::tracks_of(op_diff, &m.track);
    f.uses.extend(order.clone());
    // An explicit index (its inverse's too) holds only in the order it was chosen in.
    if let Some(order) = &order {
        f.puts_back(order);
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

/// The inverse is one `track.move`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "track.move";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TrackMove(m) => vec![&mut m.track],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TrackMove(m) => vec![&m.track],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
