//! Index placements carried by their neighbours (spec §4.3, *as built*). Some ops name a place
//! in an ordered list by its absolute index: a track's place among its timeline's tracks
//! (`track.add` with an index, `track.move`, `track.restore`), a timeline's
//! (`timeline.restore`), an effect's among its item's (`effect.add` with `at`,
//! `effect.move`, `effect.restore`), a key's in the font fallback order (`font.pin` placing
//! it). An index does not map through later list changes, so the planner reads, in the op's
//! own frame, what the index names: the members right before and after the place (`Place`;
//! a moved member is left out of its list). The engine looks for them in the head's list
//! right before the op runs and puts the op there ([`Place::resolve`]):
//! - both are still neighbours (or the list's ends are) → between them;
//! - one is gone → next to the other (right after the one before, or right before the one
//!   after);
//! - something now stands between them, nothing is left of either, or the list was empty and
//!   is not now → another author placed a member where ours goes: `SAME_PARAM`, blamed on the
//!   first record that changed the list (the list's entity, `ops::util::lists`).
//!
//! So appends and removals elsewhere in the list never stop an index op, and an undo of a
//! removal puts the member back between the neighbours it had. The stored ops keep their
//! format: only the index field is rewritten at the head. A container named by a placeholder
//! (a timeline or item made in the same transaction) holds nothing anyone else placed, and
//! is left as it is.

use crate::apply::{Applier, ApplyEnv, ApplyMode};
use crate::error::CoreError;
use crate::ids::MemIds;
use crate::model::Project;
use crate::ops::Op;
use crate::ops::util::{find_item, lists};
use crate::tx::Tx;

/// What [`Place::resolve`] finds at the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// No list there (its container is gone, or a pin's key is placed already): the op keeps
    /// its index and its re-run decides.
    Keep,
    /// The index to use.
    At(u32),
    /// Another author placed a member where ours goes.
    Taken,
}

/// Where an index op puts its member, as its own frame names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The list's entity (for blame): `<timeline>#tracks`, `#timelines`, `<item>#effects`,
    /// `#fonts.fallback`.
    pub list: String,
    /// The member the op places, left out of the list it is read from.
    member: String,
    /// The members right before and after the place (`None`: that end of the list).
    before: Option<String>,
    after: Option<String>,
}

impl Place {
    /// Where the op goes in `project`'s list (module docs).
    pub fn resolve(&self, project: &Project, op: &Op) -> Resolved {
        let Some((_, members)) = list_of(project, op) else {
            return Resolved::Keep;
        };
        let list: Vec<&String> = members.iter().filter(|m| **m != self.member).collect();
        let at = |m: &Option<String>| m.as_ref().and_then(|m| list.iter().position(|x| *x == m));
        let index = match (&self.before, &self.after) {
            (None, None) if list.is_empty() => 0,
            (Some(_), None) => match at(&self.before) {
                Some(i) if i + 1 == list.len() => i + 1,
                _ => return Resolved::Taken,
            },
            (None, Some(_)) => match at(&self.after) {
                Some(0) => 0,
                _ => return Resolved::Taken,
            },
            (Some(_), Some(_)) => match (at(&self.before), at(&self.after)) {
                (Some(b), Some(a)) if a == b + 1 => a,
                (Some(b), None) => b + 1,
                (None, Some(a)) => a,
                _ => return Resolved::Taken,
            },
            (None, None) => return Resolved::Taken,
        };
        u32::try_from(index).map_or(Resolved::Taken, Resolved::At)
    }
}

/// The place of each op of `tx` that names an index, read in its own frame: `frame` with the
/// earlier ops applied (as `mode` applies them, with fresh ids for placeholders).
pub fn places(frame: &Project, tx: &Tx, mode: ApplyMode) -> Result<Vec<Option<Place>>, CoreError> {
    if !tx.ops.iter().any(names_index) {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(tx.ops.len());
    let mut run = Applier::new(frame, tx.schema_version, mode, ApplyEnv::default())?;
    let ids = &mut MemIds::after(frame);
    for (k, op) in tx.ops.iter().enumerate() {
        out.push(place(run.project(), op));
        run.op(k, op, ids)?;
    }
    Ok(out)
}

/// Rewrites the index `op` names (module docs).
pub fn set_index(op: &mut Op, index: u32) {
    match op {
        Op::TrackAdd(a) => a.index = Some(index),
        Op::TrackMove(m) => m.index = index,
        Op::TrackRestore(r) => r.index = index,
        Op::TimelineRestore(r) => r.index = index,
        Op::EffectAdd(a) => a.at = Some(index),
        Op::EffectMove(m) => m.to = index,
        Op::EffectRestore(r) => r.index = index,
        Op::FontPin(p) => p.fallback = Some(index),
        _ => {}
    }
}

/// Whether some op of `ops` names an index (a planner reads a frame for [`places`] only then).
pub fn any(ops: &[Op]) -> bool {
    ops.iter().any(names_index)
}

/// Whether `op` names an index (and so may have a [`Place`]).
fn names_index(op: &Op) -> bool {
    match op {
        Op::TrackAdd(a) => a.index.is_some(),
        Op::EffectAdd(a) => a.at.is_some(),
        Op::FontPin(p) => p.fallback.is_some(),
        Op::TrackMove(_)
        | Op::TrackRestore(_)
        | Op::TimelineRestore(_)
        | Op::EffectMove(_)
        | Op::EffectRestore(_) => true,
        _ => false,
    }
}

/// The index `op` names, if any.
fn index_of(op: &Op) -> Option<u32> {
    match op {
        Op::TrackAdd(a) => a.index,
        Op::TrackMove(m) => Some(m.index),
        Op::TrackRestore(r) => Some(r.index),
        Op::TimelineRestore(r) => Some(r.index),
        Op::EffectAdd(a) => a.at,
        Op::EffectMove(m) => Some(m.to),
        Op::EffectRestore(r) => Some(r.index),
        Op::FontPin(p) => p.fallback,
        _ => None,
    }
}

/// The list `op` places into in `project` (its entity, and its members' ids), with the member
/// it places. `None` when the op names no index, its container is a placeholder or missing,
/// or (a pin) the key is in the order already.
fn list_of(project: &Project, op: &Op) -> Option<((String, String), Vec<String>)> {
    let literal = |id: &str| (!id.starts_with('$')).then_some(());
    let tracks = |tl: &str| -> Option<Vec<String>> {
        let t = project.timelines.iter().find(|t| t.id == tl)?;
        Some(t.tracks.iter().map(|t| t.id.clone()).collect())
    };
    let effects = |item: &str| -> Option<Vec<String>> {
        let (t, i) = find_item(project, item)?;
        let it = &t.get(project).items[i];
        Some(it.common.effects.iter().map(|e| e.id.clone()).collect())
    };
    let timeline_of = |track: &str| {
        project
            .timelines
            .iter()
            .find(|t| t.tracks.iter().any(|x| x.id == track))
            .map(|t| t.id.clone())
    };
    Some(match op {
        Op::TrackAdd(a) => {
            literal(&a.timeline)?;
            let l = (lists::tracks(&a.timeline), a.id.clone());
            (l, tracks(&a.timeline)?)
        }
        Op::TrackRestore(r) => {
            literal(&r.timeline)?;
            let l = (lists::tracks(&r.timeline), r.track.id.clone());
            (l, tracks(&r.timeline)?)
        }
        Op::TrackMove(m) => {
            let tl = timeline_of(&m.track)?;
            ((lists::tracks(&tl), m.track.clone()), tracks(&tl)?)
        }
        Op::TimelineRestore(r) => {
            let ids = project.timelines.iter().map(|t| t.id.clone()).collect();
            ((lists::TIMELINES.into(), r.timeline.id.clone()), ids)
        }
        Op::EffectAdd(a) => {
            literal(&a.item)?;
            ((lists::effects(&a.item), a.id.clone()), effects(&a.item)?)
        }
        Op::EffectMove(m) => {
            literal(&m.item)?;
            (
                (lists::effects(&m.item), m.effect.clone()),
                effects(&m.item)?,
            )
        }
        Op::EffectRestore(r) => {
            literal(&r.item)?;
            let l = (lists::effects(&r.item), r.effect.id.clone());
            (l, effects(&r.item)?)
        }
        Op::FontPin(p) => {
            if project.fonts.fallback.contains(&p.key) {
                return None;
            }
            let l = (lists::FALLBACK_ORDER.into(), p.key.clone());
            (l, project.fonts.fallback.clone())
        }
        _ => return None,
    })
}

/// The place `op` names in `project` (its own frame), if it names an index.
fn place(project: &Project, op: &Op) -> Option<Place> {
    let index = usize::try_from(index_of(op)?).ok()?;
    let ((list, member), members) = list_of(project, op)?;
    let rest: Vec<String> = members.into_iter().filter(|m| *m != member).collect();
    let index = index.min(rest.len());
    Some(Place {
        list,
        member,
        before: index.checked_sub(1).map(|i| rest[i].clone()),
        after: rest.get(index).cloned(),
    })
}
