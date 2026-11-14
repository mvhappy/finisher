//! Transition windows and which items of a track sound or show at a time (plan 4C Decisions 22
//! and 41). A transition at the cut between A and B spans `[cut − d/2, cut + d/2)`: A continues
//! past its end and B starts before its start, both reading their handles.

use std::collections::HashMap;
use std::ops::Range;

use crate::error::CoreError;
use crate::eval::interp::progress;
use crate::eval::plan::{Role, TransitionState};
use crate::model::{Curve, Item, Track, TransitionKind};
use crate::time::Rational;

/// One valid transition of a track: both items found, touching, and a positive duration
/// (anything else is skipped; replay refuses it as `TRANSITION_INVALID`). Its duration is
/// clamped to the shorter item's: a trim after `transition.set` may leave an item shorter than
/// the transition (the op checks the bound only when it is set), and the window then still
/// stays within each item's half, so an item's two windows never overlap.
#[derive(Debug, Clone)]
pub(crate) struct Window {
    pub id: String,
    pub kind: TransitionKind,
    pub curve: Curve,
    pub start: Rational,
    pub end: Rational,
    /// Indices of the outgoing and incoming item in `track.items`.
    pub out: usize,
    pub into: usize,
}

impl Window {
    pub fn contains(&self, t: Rational) -> bool {
        self.start <= t && t < self.end
    }

    pub fn state(&self, t: Rational, role: Role) -> TransitionState {
        TransitionState {
            id: self.id.clone(),
            kind: self.kind,
            curve: self.curve,
            progress: progress(self.start, self.end, t),
            role,
        }
    }
}

/// A track indexed for evaluation (plan 4C review of Tasks 15/16): its valid transition
/// windows, ordered by cut, and for each item the window it comes in by and the one it goes
/// out by. Built once per track and evaluation in `O(n + w log w)`; the items one time or range
/// can meet are then found by binary search on `start` (items are sorted by `start` and do
/// not overlap), so a frame costs no more than the items it meets.
pub(crate) struct TrackIndex<'a> {
    pub track: &'a Track,
    windows: Vec<Window>,
    /// Per item: the incoming and the outgoing window (indices into `windows`).
    roles: Vec<(Option<usize>, Option<usize>)>,
}

impl<'a> TrackIndex<'a> {
    pub fn new(track: &'a Track) -> Result<Self, CoreError> {
        let mut at: HashMap<&str, usize> = HashMap::with_capacity(track.items.len());
        for (i, item) in track.items.iter().enumerate() {
            at.entry(item.id.as_str()).or_insert(i);
        }
        let mut windows = Vec::new();
        for tr in &track.transitions {
            let (Some(&a), Some(&b)) = (
                at.get(tr.between[0].as_str()),
                at.get(tr.between[1].as_str()),
            ) else {
                continue;
            };
            let cut = track.items[b].start;
            if a == b || track.items[a].end()? != cut || !tr.duration.is_positive() {
                continue;
            }
            let shorter = track.items[a].duration.min(track.items[b].duration);
            let half = tr
                .duration
                .min(shorter)
                .checked_div(Rational::from_int(2))?;
            windows.push(Window {
                id: tr.id.clone(),
                kind: tr.kind,
                curve: tr.curve,
                start: cut.checked_sub(half)?,
                end: cut.checked_add(half)?,
                out: a,
                into: b,
            });
        }
        windows.sort_by_key(|w| w.start);
        // The first window by cut wins a role, as a linear search would find it.
        let mut roles = vec![(None, None); track.items.len()];
        for (w, win) in windows.iter().enumerate() {
            roles[win.into].0.get_or_insert(w);
            roles[win.out].1.get_or_insert(w);
        }
        Ok(TrackIndex {
            track,
            windows,
            roles,
        })
    }

    /// The windows item `i` takes part in: as the incoming item, then as the outgoing one.
    pub fn roles(&self, i: usize) -> (Option<&Window>, Option<&Window>) {
        let (into, out) = self.roles[i];
        (
            into.map(|w| &self.windows[w]),
            out.map(|w| &self.windows[w]),
        )
    }

    /// The indices of the items whose extent may meet `[from, to)` (`to` included when
    /// `closed`, for a single time): a window stays within each item's half, so only the item
    /// before the first one starting at or before `from`, and the one after the last starting
    /// before `to`, can reach in.
    pub fn candidates(&self, from: Rational, to: Rational, closed: bool) -> Range<usize> {
        let items = &self.track.items;
        let lo = items.partition_point(|i| i.start <= from).saturating_sub(2);
        let hi = match closed {
            true => items.partition_point(|i| i.start <= to),
            false => items.partition_point(|i| i.start < to),
        };
        lo..(hi + 1).min(items.len()).max(lo)
    }

    /// The items shown at `t`, in item order, each with its transition state. Clamped windows
    /// never hold an item twice at once; were one to, the incoming window would win.
    pub fn active(
        &self,
        t: Rational,
    ) -> Result<Vec<(&'a Item, Option<TransitionState>)>, CoreError> {
        let mut out = Vec::new();
        for i in self.candidates(t, t, true) {
            let item = &self.track.items[i];
            let (into, from) = self.roles(i);
            let state = match (
                into.filter(|w| w.contains(t)),
                from.filter(|w| w.contains(t)),
            ) {
                (Some(w), _) => Some(w.state(t, Role::In)),
                (None, Some(w)) => Some(w.state(t, Role::Out)),
                (None, None) => None,
            };
            if state.is_some() || (item.start <= t && t < item.end()?) {
                out.push((item, state));
            }
        }
        Ok(out)
    }
}

/// The item's extent with its transitions: from the incoming window's start (else its own
/// start) to the outgoing window's end (else its own end).
pub(crate) fn extent(
    item: &Item,
    into: Option<&Window>,
    out: Option<&Window>,
) -> Result<(Rational, Rational), CoreError> {
    let start = into.map_or(item.start, |w| w.start);
    let end = match out {
        Some(w) => w.end,
        None => item.end()?,
    };
    Ok((start, end))
}
