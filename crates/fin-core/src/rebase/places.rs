//! Index placements carried by their neighbours (spec §4.3, *as built*). Some ops name a place
//! in an ordered list by its absolute index: a track's place among its timeline's tracks
//! (`track.add` with an index, `track.move`, `track.restore`), a timeline's
//! (`timeline.restore`), an effect's among its item's (`effect.add` with `at`,
//! `effect.move`, `effect.restore`), a key's in the font fallback order (`font.pin` placing
//! it). An index does not map through later list changes, so the planner reads, in the op's
//! own frame, the list and the place the index names (`Place`; a moved member is left out of
//! its list). Right before the op runs at the head, the engine takes on each side of the place
//! the nearest frame member that is still in the list and that no other record moved or
//! re-created since (its `index` touched, or created), the list's ends counting as such
//! members ([`Slot::resolve`]):
//! - those two are neighbours at the head → the op goes between them;
//! - otherwise another author's member, or a moved one, stands where ours goes, and more than
//!   one place would keep the member's frame order with the unmoved survivors: `SAME_PARAM`,
//!   blamed on the first record that changed the list (the list's entity, `ops::util::lists`).
//!
//! So a placement keeps its frame order with every frame member that is still there and that
//! nobody moved, and is refused when that order allows more than one place: a lone surviving
//! neighbour is never trusted alone. A `font.pin` whose key someone placed since keeps its
//! index only when the key already sits where ours would put it (the same edit), else
//! `SAME_PARAM`.
//!
//! Members are named canonically on both sides: an id that an earlier op of the same
//! transaction bound is named by its placeholder (through the applier's bindings: the
//! planning frame's fresh ids for the planning side, the head's for the head), so a planning
//! id never stands for another author's entity at the head. Containers and members named by
//! a placeholder resolve the same way. An index op whose place cannot be read (`Slot::
//! Unplaced`: a pin of a key placed already in its own frame) is refused when another record
//! used its list, unless the head still holds the same file pinned (the same edit): a stale
//! absolute index is never carried silently.
//!
//! So appends and removals away from the place never stop an index op, and an undo of a
//! removal puts the member back between the neighbours it had. The stored ops keep their
//! format: only the index field is rewritten at the head.

use std::collections::BTreeMap;

use crate::apply::{Applier, ApplyEnv, ApplyMode};
use crate::error::CoreError;
use crate::ids::MemIds;
use crate::model::Project;
use crate::ops::Op;
use crate::ops::util::{find_item, lists};
use crate::tx::Tx;

/// The placeholders an applier bound so far, to the ids they name.
pub type Bound = BTreeMap<String, String>;

/// What [`Slot::resolve`] finds at the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// The op keeps its index: no list there (its container is gone: its re-run decides), or
    /// a pin whose key already sits where ours goes.
    Keep,
    /// The index to use.
    At(u32),
    /// Another author placed a member where ours goes, or the place cannot be carried.
    Taken,
}

/// What an index op names, read in its own frame (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    /// The place, by its neighbours.
    Place(Place),
    /// An index op whose place could not be read (a pin of a key placed already in its own
    /// frame): at the head it is refused when another author used `list`, and keeps its index
    /// otherwise. A stale absolute index is never carried silently.
    Unplaced { list: String },
}

impl Slot {
    /// The list's entity, for blame.
    pub fn list(&self) -> &str {
        match self {
            Slot::Place(p) => &p.list,
            Slot::Unplaced { list } => list,
        }
    }

    /// Where the op goes in `project`'s list (`bound`: the head applier's bindings), when some
    /// other record used the list (`used`) or not. `placed_since`: whether another record
    /// moved or re-created an entity since (a member id is made one by [`Place`]).
    pub fn resolve(
        &self,
        project: &Project,
        op: &Op,
        bound: &Bound,
        used: bool,
        placed_since: &dyn Fn(&str) -> bool,
    ) -> Resolved {
        match self {
            Slot::Place(p) => p.resolve(project, op, bound, placed_since),
            // A pin of a key placed already in its own frame: the same file still pinned at
            // the head is the same edit (its re-run writes nothing).
            Slot::Unplaced { .. } if same_pin(project, op) => Resolved::Keep,
            Slot::Unplaced { .. } if used => Resolved::Taken,
            Slot::Unplaced { .. } => Resolved::Keep,
        }
    }
}

/// Whether `op` is a pin of a key `project` holds pinned to the same file.
fn same_pin(project: &Project, op: &Op) -> bool {
    matches!(op, Op::FontPin(p)
        if project.fonts.pins.get(&p.key).is_some_and(|h| h.sha256 == p.pin.sha256))
}

/// Where an index op puts its member, as its own frame names it: the list there (the member
/// left out) and the index into it. Member ids are canonical: an id that an earlier op of the
/// same transaction bound is named by its placeholder, so the planning frame's fresh ids never
/// stand for another author's entities at the head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The list's entity (for blame): `<timeline>#tracks`, `#timelines`, `<item>#effects`,
    /// `#fonts.fallback`.
    pub list: String,
    /// The member the op places.
    member: String,
    /// The list in the op's own frame, without the member, and the index into it.
    frame: Vec<String>,
    index: usize,
    /// What makes a member id an entity id (`<item>/` for effects, `font/` for pins).
    entity: String,
}

impl Place {
    /// The nearest frame member on each side that is still in the list and that no other
    /// record moved or re-created since (the list's ends count as such members) must be
    /// neighbours at the head: the op goes between them. Otherwise another author's placement
    /// (or a moved member) stands where ours goes, and more than one place would keep the
    /// member's order with the others: `Taken`.
    fn resolve(
        &self,
        project: &Project,
        op: &Op,
        bound: &Bound,
        placed_since: &dyn Fn(&str) -> bool,
    ) -> Resolved {
        let Some(found) = list_of(project, op, bound) else {
            return Resolved::Keep;
        };
        let list: Vec<&String> = found
            .members
            .iter()
            .filter(|m| **m != self.member)
            .collect();
        let pos = |m: &String| list.iter().position(|x| *x == m);
        let anchor = |m: &&String| {
            pos(m).is_some() && !placed_since(&format!("{}{}", self.entity, real(m, bound)))
        };
        let left = self.frame[..self.index].iter().rev().find(anchor);
        let right = self.frame[self.index..].iter().find(anchor);
        let after_left = left.and_then(pos).map_or(0, |i| i + 1);
        let at_right = right.and_then(pos).unwrap_or(list.len());
        if after_left != at_right {
            return Resolved::Taken;
        }
        let Ok(index) = u32::try_from(at_right) else {
            return Resolved::Taken;
        };
        // A pin of a key someone placed since: kept only where ours would put it.
        match found.placed {
            Some(j) if j == at_right => Resolved::Keep,
            Some(_) => Resolved::Taken,
            None => Resolved::At(index),
        }
    }
}

/// The slot of each op of `tx` that names an index, read in its own frame: `frame` with the
/// earlier ops applied (as `mode` applies them, with fresh ids for placeholders, which the
/// slots name by their placeholders).
pub fn places(frame: &Project, tx: &Tx, mode: ApplyMode) -> Result<Vec<Option<Slot>>, CoreError> {
    if !tx.ops.iter().any(names_index) {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(tx.ops.len());
    let mut run = Applier::new(frame, tx.schema_version, mode, ApplyEnv::default())?;
    let ids = &mut MemIds::after(frame);
    for (k, op) in tx.ops.iter().enumerate() {
        out.push(slot(run.project(), op, run.bound()));
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

/// The list an index op places into, read in some project.
struct Found {
    /// The member ids, canonical ([`Place`]).
    members: Vec<String>,
    /// A pin's key already in the order: its index there.
    placed: Option<usize>,
}

/// `id` as the project names it: a placeholder an earlier op bound is its id.
fn real<'a>(id: &'a str, bound: &'a Bound) -> &'a str {
    bound.get(id).map_or(id, String::as_str)
}

/// `id` canonical: the placeholder that bound it, if an earlier op of the transaction did.
fn canonical(id: &str, bound: &Bound) -> String {
    bound
        .iter()
        .find(|(_, v)| v.as_str() == id)
        .map_or_else(|| id.to_string(), |(p, _)| p.clone())
}

/// The list's entity and member `op` places into, by the ids it names (canonical).
fn names(op: &Op) -> Option<(String, String)> {
    Some(match op {
        Op::TrackAdd(a) => (lists::tracks(&a.timeline), a.id.clone()),
        Op::TrackRestore(r) => (lists::tracks(&r.timeline), r.track.id.clone()),
        Op::TrackMove(m) => (lists::tracks(""), m.track.clone()),
        Op::TimelineRestore(r) => (lists::TIMELINES.into(), r.timeline.id.clone()),
        Op::EffectAdd(a) => (lists::effects(&a.item), a.id.clone()),
        Op::EffectMove(m) => (lists::effects(&m.item), m.effect.clone()),
        Op::EffectRestore(r) => (lists::effects(&r.item), r.effect.id.clone()),
        Op::FontPin(p) => (lists::FALLBACK_ORDER.into(), p.key.clone()),
        _ => return None,
    })
}

/// The list `op` places into in `project` (`bound`: that applier's bindings), with its entity.
/// `None` when the op names no index or its container is missing.
fn list_of(project: &Project, op: &Op, bound: &Bound) -> Option<Found> {
    let ids =
        |v: Vec<&String>| -> Vec<String> { v.into_iter().map(|m| canonical(m, bound)).collect() };
    let tracks = |tl: &str| -> Option<Vec<String>> {
        let t = project.timelines.iter().find(|t| t.id == real(tl, bound))?;
        Some(ids(t.tracks.iter().map(|t| &t.id).collect()))
    };
    let effects = |item: &str| -> Option<Vec<String>> {
        let (t, i) = find_item(project, real(item, bound))?;
        let it = &t.get(project).items[i];
        Some(ids(it.common.effects.iter().map(|e| &e.id).collect()))
    };
    let mut placed = None;
    let members = match op {
        Op::TrackAdd(a) => tracks(&a.timeline)?,
        Op::TrackRestore(r) => tracks(&r.timeline)?,
        Op::TrackMove(m) => {
            let track = real(&m.track, bound);
            let tl = project
                .timelines
                .iter()
                .find(|t| t.tracks.iter().any(|x| x.id == track))?;
            ids(tl.tracks.iter().map(|t| &t.id).collect())
        }
        Op::TimelineRestore(_) => ids(project.timelines.iter().map(|t| &t.id).collect()),
        Op::EffectAdd(a) => effects(&a.item)?,
        Op::EffectMove(m) => effects(&m.item)?,
        Op::EffectRestore(r) => effects(&r.item)?,
        Op::FontPin(p) => {
            let order = &project.fonts.fallback;
            placed = order.iter().position(|k| *k == p.key);
            order.iter().filter(|k| **k != p.key).cloned().collect()
        }
        _ => return None,
    };
    Some(Found { members, placed })
}

/// The list entity of `op` in `project` (a `track.move` names its track's timeline there).
fn list_entity(project: &Project, op: &Op, bound: &Bound) -> Option<String> {
    let (list, _) = names(op)?;
    if let Op::TrackMove(m) = op {
        let track = real(&m.track, bound);
        let tl = project
            .timelines
            .iter()
            .find(|t| t.tracks.iter().any(|x| x.id == track))?;
        return Some(lists::tracks(&canonical(&tl.id, bound)));
    }
    Some(list)
}

/// The slot `op` names in `project` (its own frame), if it names an index.
fn slot(project: &Project, op: &Op, bound: &Bound) -> Option<Slot> {
    let index = usize::try_from(index_of(op)?).ok()?;
    let list = list_entity(project, op, bound).unwrap_or_default();
    let unplaced = || Some(Slot::Unplaced { list: list.clone() });
    let Some(found) = list_of(project, op, bound) else {
        return unplaced();
    };
    if found.placed.is_some() {
        return unplaced();
    }
    let member = canonical(real(&names(op)?.1, bound), bound);
    let frame: Vec<String> = found.members.into_iter().filter(|m| *m != member).collect();
    let index = index.min(frame.len());
    let entity = match op {
        Op::EffectAdd(a) => format!("{}/", real(&a.item, bound)),
        Op::EffectMove(m) => format!("{}/", real(&m.item, bound)),
        Op::EffectRestore(r) => format!("{}/", real(&r.item, bound)),
        Op::FontPin(_) => "font/".into(),
        _ => String::new(),
    };
    Some(Slot::Place(Place {
        list: list.clone(),
        member,
        frame,
        index,
        entity,
    }))
}
