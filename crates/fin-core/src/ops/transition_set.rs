//! `transition.set`: creates or updates the transition at the cut between two touching items
//! of a track, centred on the cut (plan 4C Decision 22).
//!
//! - A `$name` id creates one (`TRANSITION_EXISTS` when the cut has one already, policy).
//! - A literal id of a transition on the track updates its kind, duration and curve; its track
//!   and cut stay (`INVALID_OP` otherwise).
//! - A literal id of none restores it (internal only: the inverse of `transition.remove` and
//!   of the post-op prune, [`crate::ops::util::prune`]).
//!
//! Structural: the items are on the track and touch (`NOT_ADJACENT`), and the duration is
//! positive. Policy: the lock; the kind fits the track (`dissolve` on video, `crossfade` on
//! audio; `TRACK_KIND_MISMATCH`); the duration is at least one frame and at most the shorter
//! item; and each clip has the handles a centred transition reads (`TRANSITION_NO_HANDLES`:
//! a clip's source past the cut, `duration / 2 × speed` on each side). A restore checks only
//! the lock and one per cut: it puts back what an earlier edit took, whatever happened to its
//! items since.
//!
//! The diff records the transition (`added` or `updated`, on its track). The footprint touches
//! `transition.out` on the outgoing item and `transition.in` on the incoming one. The inverse is
//! `transition.remove` (a create or restore) or the set of its old values (an update).

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{ItemBody, Project, Track, TrackKind, Transition, TransitionKind};
use crate::ops::util::{
    OpDiff, TrackAt, asset, check_unlocked, find_transition, frame_of, invalid_range, kind_name,
    media_duration, sort_transitions, touching, track_index, wrong_op,
};
use crate::ops::{Bindings, Op, TransitionRemove, TransitionSet};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// The parameters a transition sets on its items (plan 4C Decision 20).
pub(crate) const OUT: &str = "transition.out";
pub(crate) const IN: &str = "transition.in";

fn payload(op: &Op) -> Result<&TransitionSet, CoreError> {
    match op {
        Op::TransitionSet(s) => Ok(s),
        _ => Err(wrong_op(op)),
    }
}

/// What the set does once checked: the track, the cut, and the index of the transition it
/// updates (none for a create or restore).
struct Plan {
    track: TrackAt,
    between: [String; 2],
    update: Option<usize>,
}

fn validate(project: &Project, s: &TransitionSet, bindings: &Bindings) -> Result<Plan, CoreError> {
    let ti = track_index(project, &bindings.resolve(&s.track)?)?;
    let track = ti.get(project);
    let between = [
        bindings.resolve(&s.between[0])?,
        bindings.resolve(&s.between[1])?,
    ];
    let Some((a, b)) = touching(track, &between)? else {
        return Err(CoreError::new(
            "NOT_ADJACENT",
            format!(
                "items {} and {} do not touch on track {}",
                between[0], between[1], track.id
            ),
        )
        .with_details(json!({ "track": track.id, "between": between }))
        .with_fix("set a transition at the cut between an item and the one starting at its end"));
    };
    if !s.duration.is_positive() {
        return Err(invalid_range(
            "a transition's duration must be positive",
            json!({ "path": "duration", "duration": s.duration, "min": "0", "exclusive": true,
                    "unit": "s" }),
            "use a duration of at least one frame",
        ));
    }
    // A restore puts back what was there: of the policy, only the lock and one per cut.
    let mut restores = false;
    let update = if s.id.starts_with('$') {
        bindings.check_new(project, &s.id, IdKind::Transition)?;
        None
    } else if let Some(i) = track.transitions.iter().position(|t| t.id == s.id) {
        if track.transitions[i].between != between {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("transition {} is at another cut", s.id),
            )
            .with_details(json!({ "transition": s.id, "between": track.transitions[i].between }))
            .with_fix("remove it and set a new transition at this cut"));
        }
        Some(i)
    } else if find_transition(project, &s.id).is_some() {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("transition {} is on another track", s.id),
        )
        .with_details(json!({ "transition": s.id, "track": track.id }))
        .with_fix("remove it and set a new transition on this track"));
    } else {
        bindings.check_new(project, &s.id, IdKind::Transition)?;
        restores = true;
        None
    };
    if bindings.policy() {
        policy(
            project,
            ti,
            s,
            &between,
            (a, b),
            update.is_none(),
            !restores,
            bindings,
        )?;
    }
    Ok(Plan {
        track: ti,
        between,
        update,
    })
}

/// The policy checks of a set at the cut between items `a` and `b` (indexes on the track):
/// the lock, one per cut (`creates`), and, when `fits`, the kind, duration and handles.
#[allow(clippy::too_many_arguments)]
fn policy(
    project: &Project,
    ti: TrackAt,
    s: &TransitionSet,
    between: &[String; 2],
    (a, b): (usize, usize),
    creates: bool,
    fits: bool,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let track = ti.get(project);
    check_unlocked(track, Some(&between[0]), bindings)?;
    if creates && let Some(t) = track.transitions.iter().find(|t| t.between == *between) {
        return Err(CoreError::new(
            "TRANSITION_EXISTS",
            format!(
                "the cut between {} and {} has transition {}",
                between[0], between[1], t.id
            ),
        )
        .with_details(json!({ "transition": t.id, "between": between }))
        .with_fix(format!("update it with transition.set {}", t.id)));
    }
    if !fits {
        return Ok(());
    }
    check_kind(track, s.kind)?;
    let frame = frame_of(project, ti)?;
    let (ia, ib) = (&track.items[a], &track.items[b]);
    let max = ia.duration.min(ib.duration);
    if s.duration < frame || s.duration > max {
        return Err(invalid_range(
            format!(
                "a {} transition does not fit between {} and {}",
                s.duration, ia.id, ib.id
            ),
            json!({ "path": "duration", "duration": s.duration, "min": frame, "max": max,
                    "unit": "s" }),
            format!("use a duration between {frame} and {max}"),
        ));
    }
    let half = s.duration.checked_div(Rational::from_int(2))?;
    check_handles(project, track, a, half, true)?;
    check_handles(project, track, b, half, false)
}

fn check_kind(track: &Track, kind: TransitionKind) -> Result<(), CoreError> {
    let fits = matches!(
        (track.kind, kind),
        (TrackKind::Video, TransitionKind::Dissolve)
            | (TrackKind::Audio, TransitionKind::Crossfade)
    );
    if fits {
        return Ok(());
    }
    Err(CoreError::new(
        "TRACK_KIND_MISMATCH",
        format!(
            "a {} transition cannot go on {} track {}",
            transition_kind_name(kind),
            kind_name(track.kind),
            track.id
        ),
    )
    .with_details(json!({ "track": track.id, "trackKind": kind_name(track.kind), "kind": kind }))
    .with_fix("use dissolve on video tracks and crossfade on audio tracks"))
}

fn transition_kind_name(kind: TransitionKind) -> &'static str {
    match kind {
        TransitionKind::Crossfade => "crossfade",
        TransitionKind::Dissolve => "dissolve",
    }
}

/// The source a centred transition reads past the cut: `half` after the outgoing item's end
/// (`outgoing`), or before the incoming item's start.
fn check_handles(
    project: &Project,
    track: &Track,
    index: usize,
    half: Rational,
    outgoing: bool,
) -> Result<(), CoreError> {
    let it = &track.items[index];
    let (need, have) = match (&it.body, outgoing) {
        (
            ItemBody::Clip {
                source_in,
                speed,
                asset: a,
            },
            true,
        ) => {
            let length = media_duration(asset(project, a)?)?;
            let used = source_in.checked_add(it.duration.checked_mul(*speed)?)?;
            (half.checked_mul(*speed)?, length.checked_sub(used)?)
        }
        (
            ItemBody::Clip {
                source_in, speed, ..
            },
            false,
        ) => (half.checked_mul(*speed)?, *source_in),
        (ItemBody::Nested { source_in, .. }, false) => (half, *source_in),
        _ => return Ok(()),
    };
    if need <= have {
        return Ok(());
    }
    Err(CoreError::new(
        "TRANSITION_NO_HANDLES",
        format!(
            "item {} has {have} s of source past the cut; the transition reads {need} s",
            it.id
        ),
    )
    .with_details(json!({ "item": it.id, "need": need, "have": have,
                          "side": if outgoing { "out" } else { "in" } }))
    .with_fix("trim the items to leave source past the cut, or use a shorter transition"))
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let s = payload(op)?;
    let plan = validate(project, s, bindings)?;
    let id = match plan.update {
        Some(_) => s.id.clone(),
        None => bindings.create(project, &s.id, IdKind::Transition, ids)?,
    };
    let track = plan.track.get_mut(project);
    let track_id = track.id.clone();
    let new = Transition {
        id: id.clone(),
        between: plan.between,
        kind: s.kind,
        duration: s.duration,
        curve: s.curve,
    };
    let inverse: Op = match plan.update {
        Some(i) => {
            let old = std::mem::replace(&mut track.transitions[i], new);
            diff.record_on(&track_id, EntityKind::Transition, &id, ChangeKind::Updated);
            restore(old, &track_id).into()
        }
        None => {
            track.transitions.push(new);
            sort_transitions(track);
            diff.record_on(&track_id, EntityKind::Transition, &id, ChangeKind::Added);
            TransitionRemove { transition: id }.into()
        }
    };
    Ok(vec![inverse])
}

/// The internal set that puts transition `t` back on `track` as it was.
pub(crate) fn restore(t: Transition, track: &str) -> TransitionSet {
    TransitionSet {
        id: t.id,
        track: track.to_string(),
        between: t.between,
        kind: t.kind,
        duration: t.duration,
        curve: t.curve,
    }
}

/// Sets `transition.out` on the outgoing item and `transition.in` on the incoming one, on the
/// track it uses; creates the transition (a `$name` id; a create or restore with a literal id
/// comes from the diff, an update touches it).
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let Op::TransitionSet(s) = op else {
        return Footprint::on(None, op_diff);
    };
    let mut f = Footprint::on(Some(&s.track), op_diff);
    f.touches
        .push((s.between[0].clone(), Touch::Param(OUT.to_string())));
    f.touches
        .push((s.between[1].clone(), Touch::Param(IN.to_string())));
    if s.id.starts_with('$') {
        f.creates.push(s.id.clone());
    }
    f.uses.push(s.track.clone());
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

/// The inverse is one op.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "transition.set";
/// Whether the op is produced only as an inverse and never accepted from clients (a restore,
/// a literal id naming no transition, is internal).
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: its id, the track and the two items.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::TransitionSet(s) => {
            let TransitionSet {
                id, track, between, ..
            } = s;
            let [a, b] = between;
            vec![id, track, a, b]
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::TransitionSet(s) => vec![&s.id, &s.track, &s.between[0], &s.between[1]],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::TransitionSet(s) => Some(s.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
