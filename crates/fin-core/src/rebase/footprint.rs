//! Conflict footprints: what one op touches, per track and per entity (spec §4.3).
//!
//! A footprint comes from the resolved op plus its own diff, never from a project, so a
//! committed record is judged by what it stored. Each op module builds its own
//! (`ops::<op>::footprint`); [`footprint`] dispatches.
//!
//! Rules ([`entity_conflict`]), judged against the intervening records the caller passes (the
//! unmirrored ones, Decision 24):
//! - ours touches or uses X, and the last intervening record that removed or created X removed
//!   it → [`reasons::REMOVED`] (a stale insert of a removed asset included);
//! - the same [`Edge`] of X → [`reasons::SAME_EDGE`]; the same parameter → [`reasons::SAME_PARAM`];
//! - footprints on disjoint, non-empty track sets never conflict. A footprint with no track (an
//!   asset op) is judged by its entities alone.
//!
//! `REMOVED` wins over `SAME_EDGE`/`SAME_PARAM`; within one reason the earliest record is named.

use std::collections::BTreeSet;

use crate::ops::{self, Edge, Op};
use crate::tx::Diff;

use super::conflict::reasons;

/// How an op touches an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touch {
    /// Needs it to exist (and changes it as a whole, e.g. removes it).
    Exists,
    /// Moves one edge of it.
    Edge(Edge),
    /// Sets one parameter of it (4C).
    Param(String),
}

/// What an op touches. `tracks`: the tracks it changes or places on (with link groups, 4C:
/// every track they span); `touches`: entities it changes; `removes`/`creates`: entities it
/// takes out or brings in; `uses`: entities it refers to without changing them (an insert's
/// asset and track).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    pub tracks: BTreeSet<String>,
    pub touches: Vec<(String, Touch)>,
    pub removes: Vec<String>,
    pub creates: Vec<String>,
    pub uses: Vec<String>,
}

impl Footprint {
    /// An empty footprint on `track` (if any) and every track `diff` shifts.
    pub(crate) fn on(track: Option<&str>, diff: &Diff) -> Footprint {
        let mut tracks: BTreeSet<String> = diff.shifts.iter().map(|s| s.track.clone()).collect();
        tracks.extend(track.map(str::to_string));
        Footprint {
            tracks,
            ..Footprint::default()
        }
    }

    /// Whether the two are bound to disjoint, non-empty track sets.
    fn track_disjoint(&self, other: &Footprint) -> bool {
        !self.tracks.is_empty()
            && !other.tracks.is_empty()
            && self.tracks.is_disjoint(&other.tracks)
    }
}

/// The footprint of `op` (placeholders resolved) from its own diff.
pub fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    ops::footprint(op, op_diff)
}

/// The first entity conflict of `ours` with `theirs` (intervening records in order, as
/// `(txId, footprint)`): its reason code and the txId to blame. `None` when there is none.
pub fn entity_conflict(
    ours: &Footprint,
    theirs: &[(&str, Footprint)],
) -> Option<(&'static str, String)> {
    // Entities ours needs, each with the record that removed it last (None once re-created).
    let mut needed: Vec<(&str, Option<usize>)> = ours
        .touches
        .iter()
        .map(|(x, _)| x.as_str())
        .chain(ours.uses.iter().map(String::as_str))
        .map(|x| (x, None))
        .collect();
    needed.sort_unstable();
    needed.dedup();
    let mut same: Option<(&'static str, usize)> = None;
    for (i, (_, f)) in theirs.iter().enumerate() {
        if ours.track_disjoint(f) {
            continue;
        }
        for (x, removed_by) in needed.iter_mut() {
            if f.creates.iter().any(|c| c == x) {
                *removed_by = None;
            }
            if f.removes.iter().any(|r| r == x) {
                *removed_by = Some(i);
            }
        }
        if same.is_none() {
            same = same_touch(ours, f).map(|reason| (reason, i));
        }
    }
    // The earliest record whose removal still stands.
    let removed = needed.iter().filter_map(|(_, by)| *by).min();
    removed
        .map(|i| (reasons::REMOVED, i))
        .or(same)
        .map(|(reason, i)| (reason, theirs[i].0.to_string()))
}

/// `SAME_EDGE` or `SAME_PARAM` when both touch the same edge or parameter of one entity.
fn same_touch(ours: &Footprint, theirs: &Footprint) -> Option<&'static str> {
    ours.touches.iter().find_map(|(x, t)| {
        let reason = match t {
            Touch::Exists => return None,
            Touch::Edge(_) => reasons::SAME_EDGE,
            Touch::Param(_) => reasons::SAME_PARAM,
        };
        theirs
            .touches
            .iter()
            .any(|(y, u)| y == x && u == t)
            .then_some(reason)
    })
}
