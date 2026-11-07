//! Ordered lists as entities of their own (spec §4.3, *as built*): the track order of a
//! timeline, the timeline order, an item's effect order, the font fallback order. Every op
//! that adds, moves or removes a member lists the list's entity in its `uses`. An op that names
//! an absolute index is carried to the head by the neighbours the index names in its own frame
//! (`rebase::places`); when another author placed a member where ours goes, the conflict is
//! blamed on the first record that used the list. Appends, removals and placements elsewhere
//! in the list never meet it.
//!
//! The track order is one per timeline (`<timeline>#tracks`): `track.add` and `track.restore`
//! name their timeline, and `track.move` and `track.remove` record it on their track's change
//! (`Change::timeline`), so edits on disjoint timelines never meet here.

use crate::tx::{Diff, EntityKind};

/// The track order of `timeline`.
pub(crate) fn tracks(timeline: &str) -> String {
    format!("{timeline}#tracks")
}

/// The track order of the timeline that `own` (a `track.move`'s or `track.remove`'s own diff)
/// records for track `track`.
pub(crate) fn tracks_of(own: &Diff, track: &str) -> Option<String> {
    own.changes
        .iter()
        .find(|c| c.kind == EntityKind::Track && c.id == track)
        .and_then(|c| c.timeline.as_deref())
        .map(tracks)
}
/// The project's timeline order.
pub(crate) const TIMELINES: &str = "#timelines";
/// The project's font fallback order.
pub(crate) const FALLBACK_ORDER: &str = "#fonts.fallback";

/// The effect order of `item`.
pub(crate) fn effects(item: &str) -> String {
    format!("{item}#effects")
}
