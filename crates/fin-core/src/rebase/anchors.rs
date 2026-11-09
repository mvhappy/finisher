//! Positions carried by the item edge they were written against (spec §4.3, *as built*
//! 2026-11-09; plan `2026-11-09-rebase-anchor-mapping`).
//!
//! A position `(track, p)` an op names (`Op::positions_mut`) is read in the op's own frame (its
//! earlier ops applied) as an [`Anchor`]:
//! - `start`: the item on the track that starts at `p` (at a cut, the item that follows it:
//!   `Assoc::After` sticks to what follows);
//! - `end`: the item that ends at `p`;
//! - `empty`: the track holds no items there.
//!
//! A position in a gap away from every edge, or inside an item, names a time, not an item, and
//! has no anchor; so does a lift `item.restore`, which refills the exact span it was taken from.
//! Those map through `Diff.shifts`, as every position did before.
//!
//! Right before the op re-runs at the head ([`Anchor::resolve`]), `start` resolves to that item's
//! start there, else `end` to its end, from where a walk passes what the head inserted at that
//! point since ([`Placed`]: what `Assoc::After` passes), else `empty` to `p` while the track is
//! still empty. An anchor removed, or moved to another track, is gone; with none left the
//! position maps through shifts (the op keeps the track it names). Only the position fields are
//! rewritten at the head: the stored ops and inverses stay absolute, as with `rebase::places`.
//!
//! Items are named canonically, as `places` names list members: an id an earlier op of the same
//! transaction bound is named by its placeholder, so a planning id never stands for another
//! author's item at the head.

use std::collections::{BTreeMap, BTreeSet};

use crate::apply::{Applier, ApplyEnv, ApplyMode};
use crate::error::CoreError;
use crate::ids::MemIds;
use crate::model::{Project, Track};
use crate::ops::util::tracks;
use crate::ops::{DeleteMode, InsertMode, Op};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, Tx};

use super::places::{Bound, canonical, real};

/// What a position touches in its op's own frame (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// The item starting at the position (canonical id).
    pub start: Option<String>,
    /// The item ending at the position (canonical id).
    pub end: Option<String>,
    /// The track was empty: the position as written.
    pub empty: Option<Rational>,
}

/// Per op, per position (in `Op::positions_mut` order), its anchor; `None` maps by shifts.
pub type Anchors = Vec<Vec<Option<Anchor>>>;

/// Whether some op of `ops` names a position (a planner reads a frame for [`anchors`] only then).
pub fn any(ops: &[Op]) -> bool {
    ops.iter().any(|op| !op.clone().positions_mut().is_empty())
}

/// The anchors of each op of `tx`, read in its own frame: `frame` with the earlier ops applied
/// (as `mode` applies them, with fresh ids for placeholders, which anchors name by their
/// placeholders). Empty when no op names a position.
pub fn anchors(frame: &Project, tx: &Tx, mode: ApplyMode) -> Result<Anchors, CoreError> {
    if !any(&tx.ops) {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(tx.ops.len());
    let mut run = Applier::new(frame, tx.schema_version, mode, ApplyEnv::default())?;
    let ids = &mut MemIds::after(frame);
    for (k, op) in tx.ops.iter().enumerate() {
        out.push(of_op(run.project(), op, run.bound()));
        run.op(k, op, ids)?;
    }
    Ok(out)
}

/// The anchors of `op`'s positions in `project` (its own frame).
fn of_op(project: &Project, op: &Op, bound: &Bound) -> Vec<Option<Anchor>> {
    let lift = matches!(op, Op::ItemRestore(r) if r.mode == DeleteMode::Lift);
    let mut op = op.clone();
    op.positions_mut()
        .into_iter()
        .map(|(track, p)| {
            if lift {
                return None;
            }
            let t = track_of(project, real(&track, bound))?;
            of_position(t, *p, bound)
        })
        .collect()
}

/// The anchor of position `p` on `t`.
fn of_position(t: &Track, p: Rational, bound: &Bound) -> Option<Anchor> {
    if t.items.is_empty() {
        return Some(Anchor {
            start: None,
            end: None,
            empty: Some(p),
        });
    }
    let start = t.items.iter().find(|i| i.start == p);
    let end = t.items.iter().find(|i| i.end().is_ok_and(|e| e == p));
    if start.is_none() && end.is_none() {
        return None;
    }
    Some(Anchor {
        start: start.map(|i| canonical(&i.id, bound)),
        end: end.map(|i| canonical(&i.id, bound)),
        empty: None,
    })
}

fn track_of<'a>(project: &'a Project, id: &str) -> Option<&'a Track> {
    tracks(project).find(|t| t.id == id)
}

/// What [`Anchor::resolve`] reads besides the head: the placeholders the head applier bound,
/// the ids our own earlier ops created there, the right parts our earlier splits cut (by the
/// item each cut, both as head ids), and what the head placed since ([`Placed`]).
pub(crate) struct Ours<'a> {
    pub bound: &'a Bound,
    pub created: &'a BTreeSet<String>,
    pub parts: BTreeMap<String, String>,
    pub placed: &'a Placed,
}

impl Ours<'_> {
    /// Whether our transaction made item `id` (as the head names it).
    fn made(&self, id: &str) -> bool {
        self.bound.values().any(|v| v == id)
            || self.created.contains(id)
            || self.created.contains(&canonical(id, self.bound))
    }
}

/// The right parts our earlier ops (`ops`, already re-run) cut by `item.split`, by the item each
/// cut, as the head names them.
pub(crate) fn split_parts(ops: &[Op], bound: &Bound) -> BTreeMap<String, String> {
    ops.iter()
        .filter_map(|op| match op {
            Op::ItemSplit(s) => Some((
                real(&s.id, bound).to_string(),
                real(&s.item, bound).to_string(),
            )),
            _ => None,
        })
        .collect()
}

impl Anchor {
    /// Where the position is at the head (`project`, on `track` as the op names it), or `None`
    /// when no anchor is left there (the caller maps by shifts).
    pub(crate) fn resolve(
        &self,
        project: &Project,
        track: &str,
        ours: &Ours<'_>,
    ) -> Option<Rational> {
        let t = track_of(project, real(track, ours.bound))?;
        let on = |name: &str| {
            let id = real(name, ours.bound);
            t.items.iter().find(|i| i.id == id)
        };
        if let Some(b) = self.start.as_deref().and_then(on) {
            return Some(b.start);
        }
        if let Some(a) = self.end.as_deref().and_then(on) {
            return walk(t, a.end().ok()?, ours);
        }
        match self.empty {
            Some(p) if t.items.is_empty() => Some(p),
            _ => None,
        }
    }
}

/// From `at` (an anchor's end), past each item the head inserted at the point since (`Assoc::
/// After`), and past the right part our own split cut from an item passed.
fn walk(t: &Track, mut at: Rational, ours: &Ours<'_>) -> Option<Rational> {
    let mut last: Option<&str> = None;
    while let Some(n) = t.items.iter().find(|n| {
        n.start == at
            && (!ours.made(&n.id) && ours.placed.passes(&n.id)
                || last.is_some_and(|l| ours.parts.get(&n.id).map(String::as_str) == Some(l)))
    }) {
        at = n.end().ok()?;
        last = Some(&n.id);
    }
    Some(at)
}

/// What committed records placed since a frame (spec §4.3, the `End` walk): the items they
/// ripple-moved (start changed by an `item.move` in ripple mode), created (`Added`, not removed
/// by an earlier record), and relocated without a shift (moved or slid, or an edge or start
/// changed with no shift of that op on its track). One step's facts ([`Placed::of_step`]) are
/// joined in step order over the steps in no mirrored pair ([`Placed::join`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placed {
    ripple: BTreeSet<String>,
    added: BTreeSet<String>,
    removed: BTreeSet<String>,
    relocated: BTreeSet<String>,
}

impl Placed {
    /// One record's facts (`ops` with placeholders resolved, `diff` its diff). An `invert`ed
    /// step stands for the record undone: `Added` and `Removed` swap, and its moves relocate.
    pub fn of_step(ops: &[Op], diff: &Diff, invert: bool) -> Placed {
        let mut p = Placed::default();
        for ch in &diff.changes {
            let Some(op) = usize::try_from(ch.op).ok().and_then(|i| ops.get(i)) else {
                continue;
            };
            match (&ch.change, invert) {
                (ChangeKind::Added, false) | (ChangeKind::Removed, true) => {
                    p.added.insert(ch.id.clone());
                }
                (ChangeKind::Removed, false) | (ChangeKind::Added, true) => {
                    p.removed.insert(ch.id.clone());
                }
                _ => {}
            }
            let start = ch.path.as_deref() == Some("start");
            if let Op::ItemMove(m) = op
                && m.mode == InsertMode::Ripple
                && start
                && !invert
            {
                p.ripple.insert(ch.id.clone());
            }
            if matches!(op, Op::ItemMove(_) | Op::ItemSlide(_)) {
                p.relocated.insert(ch.id.clone());
            }
            let shifted = diff
                .shifts
                .iter()
                .any(|x| x.op == ch.op && Some(&x.track) == ch.track.as_ref());
            if (ch.edge.is_some() || start) && !shifted {
                p.relocated.insert(ch.id.clone());
            }
        }
        p
    }

    /// `steps` (in order) joined: an id an earlier step removed is not created by a later one
    /// (it is put back, not new).
    pub fn join<'a>(steps: impl IntoIterator<Item = &'a Placed>) -> Placed {
        let mut out = Placed::default();
        for s in steps {
            out.ripple.extend(s.ripple.iter().cloned());
            out.relocated.extend(s.relocated.iter().cloned());
            for id in &s.added {
                if !out.removed.contains(id) {
                    out.added.insert(id.clone());
                }
            }
            out.removed.extend(s.removed.iter().cloned());
        }
        out
    }

    /// Whether the `End` walk passes item `id` (not ours): a ripple move put it, or it was
    /// created since and not relocated without a shift since.
    pub fn passes(&self, id: &str) -> bool {
        self.ripple.contains(id) || self.added.contains(id) && !self.relocated.contains(id)
    }
}
