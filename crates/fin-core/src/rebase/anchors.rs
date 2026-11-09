//! Positions carried by the item edge they were written against (spec §4.3, *as built*
//! 2026-11-09; plan `2026-11-09-rebase-anchor-mapping`).
//!
//! A position `(track, p)` an op names (`Op::positions_mut`) is read in the op's own frame (its
//! earlier ops applied) as an [`Anchor`]:
//! - `start`: the item on the track that starts at `p` (at a cut, the item that follows it:
//!   what is inserted at the cut goes before us, `Assoc::After`);
//! - `track_end`: no item starts at `p` and it is the end of the track's last item, so it is
//!   the track's end (an append: what the head appended since goes before us);
//! - `end`: the item that ends at `p` (with a gap after it, or as the fallback of `start`); at
//!   the head only while nothing starts at that end (else shifts decide: they pass it);
//! - `empty`: the track holds no items there.
//!
//! A position in a gap away from every edge, or inside an item, names a time, not an item, and
//! has no anchor; so does a lift `item.restore`, which refills the exact span it was taken from.
//! Those map through `Diff.shifts`, as every position did before.
//!
//! Right before the op re-runs at the head ([`Anchor::resolve`]), `start` resolves to that item's
//! start there, else `track_end` to the end of the track's last item, else `end` to that item's
//! end, else `empty` to `p` while the track is still empty. An item removed, or moved to another
//! track, is gone; with nothing left the position maps through shifts (the op keeps the track it
//! names). Only the position fields are rewritten at the head: the stored ops and inverses stay
//! absolute, as with `rebase::places`.
//!
//! Items are named canonically, as `places` names list members: an id an earlier op of the same
//! transaction bound is named by its placeholder, so a planning id never stands for another
//! author's item at the head.

use crate::apply::{Applier, ApplyEnv, ApplyMode};
use crate::error::CoreError;
use crate::ids::MemIds;
use crate::model::{Project, Track};
use crate::ops::util::tracks;
use crate::ops::{DeleteMode, Op};
use crate::time::Rational;
use crate::tx::Tx;

use super::places::{Bound, canonical, real};

/// What a position touches in its op's own frame (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// The item starting at the position (canonical id).
    pub start: Option<String>,
    /// The item ending at the position (canonical id).
    pub end: Option<String>,
    /// No item starts at the position and it is the end of the track's last item.
    pub track_end: bool,
    /// The track was empty: the position as written.
    pub empty: Option<Rational>,
}

/// Per op, per position (in `Op::positions_mut` order), its anchor; `None` maps by shifts.
pub type Anchors = Vec<Vec<Option<Anchor>>>;

/// Whether some op of `ops` names a position (a planner reads a frame for [`anchors`] only then).
pub fn any(ops: &[Op]) -> bool {
    ops.iter().any(positional)
}

/// Whether `op` is one of the ops `Op::positions_mut` lists a position of (an insert, a move,
/// a restore and the `clip.insert` alias), without copying it.
pub(crate) fn positional(op: &Op) -> bool {
    matches!(
        op,
        Op::ItemInsert(_) | Op::ItemMove(_) | Op::ItemRestore(_) | Op::ClipInsert { .. }
    )
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
    if !positional(op) {
        return Vec::new();
    }
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
            track_end: false,
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
        track_end: start.is_none() && last_end(t) == Some(p),
        empty: None,
    })
}

fn track_of<'a>(project: &'a Project, id: &str) -> Option<&'a Track> {
    tracks(project).find(|t| t.id == id)
}

/// The end of `t`'s last item, if it holds any.
fn last_end(t: &Track) -> Option<Rational> {
    t.items.iter().filter_map(|i| i.end().ok()).max()
}

impl Anchor {
    /// Where the position is at the head (`project`, on `track` as the op names it), or `None`
    /// when no anchor is left there (the caller maps by shifts).
    pub(crate) fn resolve(
        &self,
        project: &Project,
        track: &str,
        bound: &Bound,
    ) -> Option<Rational> {
        let t = track_of(project, real(track, bound))?;
        let on = |name: &str| {
            let id = real(name, bound);
            t.items.iter().find(|i| i.id == id)
        };
        if let Some(b) = self.start.as_deref().and_then(on) {
            return Some(b.start);
        }
        if self.track_end
            && let Some(end) = last_end(t)
        {
            return Some(end);
        }
        // An end followed by a gap where written. When the head put something at that end
        // since, which side of it ours goes has no intent signal, and an overwrite there would
        // cut into another author's content unannounced: the position maps through shifts,
        // which pass what was inserted there (implementation review, 2026-11-09).
        if let Some(a) = self.end.as_deref().and_then(on) {
            let end = a.end().ok()?;
            return (!t.items.iter().any(|n| n.start == end)).then_some(end);
        }
        match self.empty {
            Some(p) if t.items.is_empty() => Some(p),
            _ => None,
        }
    }
}
