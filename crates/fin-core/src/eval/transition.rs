//! Transition windows and which items of a track sound or show at a time (plan 4C Decisions 22
//! and 41). A transition at the cut between A and B spans `[cut − d/2, cut + d/2)`: A continues
//! past its end and B starts before its start, both reading their handles.

use crate::error::CoreError;
use crate::eval::interp::progress;
use crate::eval::plan::{Role, TransitionState};
use crate::model::{Curve, Item, Track, TransitionKind};
use crate::time::Rational;

/// One valid transition of a track: both items found, touching, and a positive duration
/// (anything else is skipped; replay refuses it as `TRANSITION_INVALID`).
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

/// The track's valid transition windows, ordered by cut.
pub(crate) fn windows(track: &Track) -> Result<Vec<Window>, CoreError> {
    let index = |id: &str| track.items.iter().position(|i| i.id == id);
    let mut out = Vec::new();
    for tr in &track.transitions {
        let (Some(a), Some(b)) = (index(&tr.between[0]), index(&tr.between[1])) else {
            continue;
        };
        let cut = track.items[b].start;
        if a == b || track.items[a].end()? != cut || !tr.duration.is_positive() {
            continue;
        }
        let half = tr.duration.checked_div(Rational::from_int(2))?;
        out.push(Window {
            id: tr.id.clone(),
            kind: tr.kind,
            curve: tr.curve,
            start: cut.checked_sub(half)?,
            end: cut.checked_add(half)?,
            out: a,
            into: b,
        });
    }
    out.sort_by_key(|w| w.start);
    Ok(out)
}

/// The windows an item takes part in: as the incoming item, then as the outgoing one.
pub(crate) fn roles(windows: &[Window], i: usize) -> (Option<&Window>, Option<&Window>) {
    (
        windows.iter().find(|w| w.into == i),
        windows.iter().find(|w| w.out == i),
    )
}

/// The items of `track` shown at `t`, in item order, each with its transition state. An item
/// in two windows at once (a transition longer than the item) takes the earlier one.
pub(crate) fn active(
    track: &Track,
    t: Rational,
) -> Result<Vec<(&Item, Option<TransitionState>)>, CoreError> {
    let windows = windows(track)?;
    let mut out = Vec::new();
    for (i, item) in track.items.iter().enumerate() {
        let (into, from) = roles(&windows, i);
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
