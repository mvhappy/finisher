//! Ordered lists as entities of their own (spec §4.3, *as built*). An absolute index holds
//! only in the order it was chosen in: it never maps through later placements. An inverse that
//! puts a member back where it was (`track.restore`, `timeline.restore`, `effect.restore`, a
//! move back, `font.pin` placing a key in the fallback order) would otherwise reorder another
//! author's list without a word, and so would a stale client op that names an index
//! (`track.add` or `effect.add` with one, `track.move`, `effect.move`). Every op that adds,
//! moves or removes a member lists the list's entity in its `uses`, and every op that names an
//! index declares the list put back (`Footprint::puts_back`), so a placement since then meets
//! it as `SAME_PARAM`. Appends and removals name no index: they still merge with each other,
//! since `uses` is never judged against `uses`.
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
