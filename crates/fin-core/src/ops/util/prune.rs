//! The post-op transition pass (plan 4C Decision 22): after every op but `transition.*`, in
//! every apply mode, on the tracks the op touched ([`touched_tracks`], Decision 25):
//!
//! - each transition whose items no longer touch is removed (`removed`), and its restore is
//!   appended after the op's own inverse, so an undo puts the items back first;
//! - each transition that the op's own inverse would prune on its way, although the inverse
//!   ends with its items touching again, is **carried**: recorded `updated`, removed before
//!   that inverse and restored after it. Only an inverse of several ops passes through such a
//!   state (an overwrite trim's strict trim back separates the cut before the carved
//!   neighbour's trim back closes it; a move's remove and restore); a dry run of it on a copy
//!   finds them, so undo stays exact.

use std::cmp::Ordering;

use serde_json::json;

use crate::apply::ApplyMode;
use crate::error::CoreError;
use crate::ids::MemIds;
use crate::model::{Project, Timeline, Track, Transition};
use crate::ops::transition_set::restore;
use crate::ops::util::{OpDiff, TrackAt, end, find_transition, touched_tracks};
use crate::ops::{Bindings, Op, TransitionRemove};
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Runs the pass after an op that recorded into `d` and returned `inverse`: removes the
/// transitions whose items no longer touch, carries the ones `inverse` would prune, and
/// returns the whole inverse: the carried removes, `inverse`, the restores of the pruned
/// ones, then the restores of the carried ones.
pub(crate) fn prune(
    project: &mut Project,
    d: &mut OpDiff<'_>,
    inverse: Vec<Op>,
) -> Result<Vec<Op>, CoreError> {
    let restores = remove_apart(project, d)?;
    let carried = if inverse.len() > 1 {
        carried(project, &d.own(), &inverse)
    } else {
        Vec::new()
    };
    let mut out = Vec::with_capacity(inverse.len() + restores.len() + 2 * carried.len());
    for (track, t) in &carried {
        d.record_on(track, EntityKind::Transition, &t.id, ChangeKind::Updated);
        out.push(
            TransitionRemove {
                transition: t.id.clone(),
            }
            .into(),
        );
    }
    out.extend(inverse);
    out.extend(restores);
    out.extend(
        carried
            .into_iter()
            .map(|(track, t)| restore(t, &track).into()),
    );
    Ok(out)
}

/// Removes the transitions whose items no longer touch on the tracks `d`'s op touched,
/// recording each (`removed`); returns their restores.
fn remove_apart(project: &mut Project, d: &mut OpDiff<'_>) -> Result<Vec<Op>, CoreError> {
    let own = d.own();
    let mut restores = Vec::new();
    for ti in touched_tracks(project, &own) {
        let track = ti.get_mut(project);
        let mut i = 0;
        while i < track.transitions.len() {
            if touching(track, &track.transitions[i].between)?.is_some() {
                i += 1;
                continue;
            }
            let t = track.transitions.remove(i);
            d.record_on(
                &track.id,
                EntityKind::Transition,
                &t.id,
                ChangeKind::Removed,
            );
            restores.push(restore(t, &track.id).into());
        }
    }
    Ok(restores)
}

/// The transitions (with their tracks) that applying `inverse` op by op to `project` would
/// prune: a dry run on a copy, structure only. Skipped (none) when no track the op touched
/// (`own`) holds a transition, and when the dry run fails (an inverse always applies to the
/// state its op left).
fn carried(project: &Project, own: &Diff, inverse: &[Op]) -> Vec<(String, Transition)> {
    let touched = touched_tracks(project, own);
    if touched
        .iter()
        .all(|t| t.get(project).transitions.is_empty())
    {
        return Vec::new();
    }
    let mut copy = touched_only(project, &touched);
    let mut bindings = Bindings::new(ApplyMode::Replay);
    let mut ids = MemIds::after(project);
    let mut diff = Diff::default();
    for (i, op) in inverse.iter().enumerate() {
        let mut d = OpDiff::new(&mut diff, u32::try_from(i).unwrap_or(u32::MAX));
        let ran = crate::ops::module_apply(&mut copy, op, &mut bindings, &mut ids, &mut d);
        if ran.is_err() || (crate::ops::prunes(op) && remove_apart(&mut copy, &mut d).is_err()) {
            return Vec::new();
        }
    }
    diff.changes
        .iter()
        .filter(|c| c.kind == EntityKind::Transition && c.change == ChangeKind::Removed)
        .filter_map(|c| {
            let at = find_transition(project, &c.id)?;
            let track = at.get(project);
            let t = track.transitions.iter().find(|t| t.id == c.id)?;
            Some((track.id.clone(), t.clone()))
        })
        .collect()
}

/// A copy of `project` holding, of its timelines' contents, only the `touched` tracks (an
/// inverse touches only the tracks its op touched; assets and styles stay, for the structural
/// checks), so the dry run does not clone every track of a large project.
fn touched_only(project: &Project, touched: &[TrackAt]) -> Project {
    let timelines = project
        .timelines
        .iter()
        .enumerate()
        .map(|(i, tl)| Timeline {
            id: tl.id.clone(),
            name: tl.name.clone(),
            rate: tl.rate,
            resolution: tl.resolution,
            markers: Vec::new(),
            tracks: touched
                .iter()
                .filter(|t| t.timeline == i)
                .map(|t| t.get(project).clone())
                .collect(),
        })
        .collect();
    Project {
        id: project.id.clone(),
        name: project.name.clone(),
        schema_version: project.schema_version,
        rate: project.rate,
        timelines,
        bins: project.bins.clone(),
        assets: project.assets.clone(),
        styles: project.styles.clone(),
        fonts: project.fonts.clone(),
    }
}

/// The indexes of items `between` on `track` when the first ends where the second starts and
/// they are neighbours there; `None` otherwise (either missing included).
pub(crate) fn touching(
    track: &Track,
    between: &[String; 2],
) -> Result<Option<(usize, usize)>, CoreError> {
    let find = |id: &str| track.items.iter().position(|c| c.id == id);
    let (Some(a), Some(b)) = (find(&between[0]), find(&between[1])) else {
        return Ok(None);
    };
    Ok((b == a + 1 && end(&track.items[a])? == track.items[b].start).then_some((a, b)))
}

/// Keeps a track's transitions in id order (`tr2` before `tr10`), so a removed transition is
/// restored where it was.
pub(crate) fn sort_transitions(track: &mut Track) {
    track
        .transitions
        .sort_by(|x, y| match x.id.len().cmp(&y.id.len()) {
            Ordering::Equal => x.id.cmp(&y.id),
            other => other,
        });
}

/// The structural invariant of a track's transitions (replay's post-op check): each is at a
/// cut between two touching items, with a positive duration, one per cut
/// (`TRANSITION_INVALID`).
pub(crate) fn check_transitions(track: &Track) -> Result<(), CoreError> {
    for (i, t) in track.transitions.iter().enumerate() {
        let dup = track.transitions[..i]
            .iter()
            .any(|u| u.between == t.between);
        let why = if touching(track, &t.between)?.is_none() {
            Some("its items do not touch")
        } else if !t.duration.is_positive() {
            Some("its duration is not positive")
        } else if dup {
            Some("its cut has another transition")
        } else {
            None
        };
        if let Some(why) = why {
            return Err(CoreError::new(
                "TRANSITION_INVALID",
                format!("transition {} on track {}: {why}", t.id, track.id),
            )
            .with_details(json!({ "transition": t.id, "track": track.id, "between": t.between })));
        }
    }
    Ok(())
}
