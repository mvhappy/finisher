//! The tracks an op's own diff touched (plan 4C Decision 25, blocker B1): replay checks each
//! of them after the op, so an op that moves items without recording a shift (a lift
//! restore, a strict trim) is checked too.

use crate::model::Project;
use crate::ops::util::{TrackAt, find_item, find_transition, track_index};
use crate::tx::{Diff, EntityKind};

/// Every track `own` touched, in `project` as the op left it, each once, in first-seen
/// order: the tracks its shifts name, the tracks holding an item or transition one of its
/// changes names, and the tracks a track change names. Entities the op removed are on no
/// track and add none.
pub(crate) fn touched_tracks(project: &Project, own: &Diff) -> Vec<TrackAt> {
    let mut out: Vec<TrackAt> = Vec::new();
    let mut add = |t: Option<TrackAt>| {
        if let Some(t) = t
            && !out.contains(&t)
        {
            out.push(t);
        }
    };
    for s in &own.shifts {
        add(track_index(project, &s.track).ok());
    }
    for c in &own.changes {
        match c.kind {
            EntityKind::Item => add(find_item(project, &c.id).map(|(t, _)| t)),
            EntityKind::Track => add(track_index(project, &c.id).ok()),
            EntityKind::Transition => add(find_transition(project, &c.id)),
            _ => {}
        }
    }
    out
}
