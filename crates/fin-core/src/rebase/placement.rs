//! Where an undo, redo or copy puts an item back (owner decisions of 2026-11-09; SP4 §4.3
//! *Placement of restored items*; plan `2026-11-09-rebase-anchor-mapping`, *Owner decisions*).
//!
//! On the history paths (a [`super::Rebase`] with `history` set), a ripple `item.restore` and a
//! ripple `item.move` ([`left_first`]) resolve their position **left neighbour first**:
//! 1. `end`, the item A whose end touched the position in the op's own frame: right after A,
//!    wherever A is now, whatever starts there since;
//! 2. else `start`, the item C that started there: before C;
//! 3. else `empty`: the position as written, while the track is still empty;
//! 4. else the position is **lost**: it maps through shifts (the old time, else the position as
//!    written), then [`WhenLost`] decides: `Snap` puts it at the nearest boundary on the track,
//!    `Refuse` is the conflict `NEIGHBOURS_GONE` (an agent decides).
//!
//! An item the op moves is never its own neighbour. A position with no neighbour written (a
//! gap away from every edge) maps through shifts, as on every other path.
//!
//! Each such op reports a [`Placement`]: `clean` when the neighbour written first resolved to
//! the time the shifts give (it moved only with the ripples around it), else `relocated`, with
//! where the item landed and the record that moved or removed each neighbour (`causes`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::anchors::Anchor;
use super::conflict::{Conflict, reasons};
use super::footprint::{Footprint, Touch};
use super::places::{Bound, real};
use crate::model::{Item, Project, Track};
use crate::ops::util::tracks;
use crate::ops::{DeleteMode, Edge, InsertMode, Op};
use crate::time::Rational;

/// What a history op does with an item whose neighbours are both gone (module docs). The
/// store sets it per call from who asked (a person: `Snap`; an agent: `Refuse`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhenLost {
    /// Put it back at the old time, snapped to the nearest boundary (`relocated`).
    #[default]
    Snap,
    /// Refuse the op (`NEIGHBOURS_GONE`): the caller decides.
    Refuse,
}

/// The placement outcome of one restored item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Clean,
    Relocated,
    Conflict,
}

/// How a neighbour changed since the op was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Change {
    Moved,
    Removed,
}

/// Where a relocated item landed: its track and start, the start as a whole frame of its
/// timeline (when it is one), and its neighbours there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Landed {
    pub track: String,
    pub at: Rational,
    // The optional fields have no doc comment of their own (see `tx::Change`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "i64")]
    pub frame: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub before: Option<String>,
}

/// Why an item was relocated: the committed record that last moved or removed `neighbour`,
/// and its author when the log knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cause {
    pub neighbour: String,
    pub change: Change,
    pub tx_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub author: Option<String>,
}

/// The placement of one item an undo, redo or copy put back (module docs): which op of which
/// record, the item, the outcome, and for a relocation where it landed and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub tx_id: String,
    pub op_index: usize,
    pub item: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Landed")]
    pub landed: Option<Landed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub causes: Vec<Cause>,
}

/// Whether `op` takes the left-first rule on a history path: a ripple restore or ripple move.
pub(crate) fn left_first(op: &Op) -> bool {
    match op {
        Op::ItemRestore(r) => r.mode == DeleteMode::Ripple,
        Op::ItemMove(m) => m.mode == InsertMode::Ripple,
        _ => false,
    }
}

/// The item a [`left_first`] op puts back.
pub(crate) fn item_of(op: &Op) -> Option<String> {
    match op {
        Op::ItemRestore(r) => Some(r.item.id.clone()),
        Op::ItemMove(m) => Some(m.item.clone()),
        _ => None,
    }
}

/// What the engine holds while it resolves op `k`'s position: the evolving head, the
/// placeholders bound so far, the unmirrored steps' footprints, and the policy.
pub(crate) struct Ctx<'a> {
    pub project: &'a Project,
    pub bound: &'a Bound,
    pub theirs: &'a [(&'a str, &'a Footprint)],
    pub when: WhenLost,
    pub item: &'a str,
    /// The placements of our earlier ops (a neighbour one of them put back).
    pub earlier: &'a [Placement],
}

/// A position resolved: its outcome so far and its causes, until the op has re-run.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    outcome: Outcome,
    causes: Vec<Cause>,
}

impl Pending {
    /// The refusal this placement stands for, if it is one (`WhenLost::Refuse`).
    pub(crate) fn refusal(&self, tx_id: &str, k: usize, item: &str) -> Option<Conflict> {
        (self.outcome == Outcome::Conflict).then(|| refused(tx_id, k, item, self.causes.clone()))
    }
}

impl Pending {
    /// A position that mapped by shifts: nothing written to resolve, so nothing moved.
    pub(crate) fn clean() -> Pending {
        Pending {
            outcome: Outcome::Clean,
            causes: Vec::new(),
        }
    }

    /// The placement of the item `item` once op `k` of `tx_id` re-ran at `project`.
    pub(crate) fn finish(self, project: &Project, tx_id: &str, k: usize, item: &str) -> Placement {
        let landed = (self.outcome == Outcome::Relocated)
            .then(|| landed(project, item))
            .flatten();
        Placement {
            tx_id: tx_id.to_string(),
            op_index: k,
            item: item.to_string(),
            outcome: self.outcome,
            landed,
            causes: self.causes,
        }
    }
}

/// The left-first position of `(track, written)` at the head (module docs). `shifted`: the
/// position mapped through shifts (`None` when they say it was removed). `Ok(None)` when no
/// neighbour was written, or the track is gone: the caller maps by shifts (and the re-run
/// meets the missing track, `NOT_FOUND`, for everyone).
///
/// The outcome is `clean` when the neighbour that decided is the one written first (A, else
/// C, else the empty track or the track start) and no unmirrored record moved or removed it
/// since: a concurrent insert next to it changes nothing. A neighbour an earlier op of ours put
/// back passes its own outcome on (`earlier`). A refusal is not raised here: the pending
/// placement says so, and the engine raises it once the entity rules and the re-run have
/// passed, so a gone asset or track keeps its own reason.
pub(crate) fn resolve(
    ctx: &Ctx<'_>,
    track: &str,
    written: Rational,
    a: &Anchor,
    shifted: Option<Rational>,
) -> Option<(Rational, Pending)> {
    let not_self = |n: &Option<String>| n.clone().filter(|n| real(n, ctx.bound) != ctx.item);
    let (left, right) = (not_self(&a.end), not_self(&a.start));
    if left.is_none() && right.is_none() && a.empty.is_none() {
        return None;
    }
    let t = tracks(ctx.project).find(|t| t.id == real(track, ctx.bound))?;
    let on = |n: &str| t.items.iter().find(|i| i.id == real(n, ctx.bound));
    // The track start stands in for the left neighbour of an item that was first, before C.
    let first = left.is_none() && right.is_some() && written.is_zero();
    let resolved = if let Some(at) = left.as_deref().and_then(on).and_then(|i| i.end().ok()) {
        Some((at, left.clone()))
    } else if let Some(b) = right.as_deref().and_then(on) {
        Some((b.start, right.clone().filter(|_| left.is_none())))
    } else if first {
        Some((Rational::zero(), None))
    } else {
        a.empty.filter(|_| t.items.is_empty()).map(|p| (p, None))
    };
    if let Some((at, by)) = resolved {
        let primary = match (&by, &left, &right) {
            (Some(_), _, _) => true,
            (None, None, None) => true,
            (None, None, Some(_)) => first,
            _ => false,
        };
        let mut causes = Vec::new();
        if !primary && let Some(l) = left.as_deref().filter(|l| on(l).is_none()) {
            causes.extend(cause(ctx, l, false, Edge::End));
        }
        let mut inherited = false;
        if let Some(n) = &by {
            let id = real(n, ctx.bound);
            if let Some(p) = ctx.earlier.iter().find(|p| p.item == id) {
                inherited = p.outcome != Outcome::Clean;
                causes.extend(p.causes.iter().cloned());
            }
            let edge = if left.as_ref() == Some(n) {
                Edge::End
            } else {
                Edge::Start
            };
            causes.extend(cause(ctx, n, true, edge));
        }
        let outcome = if primary && causes.is_empty() && !inherited {
            Outcome::Clean
        } else {
            Outcome::Relocated
        };
        return Some((at, Pending { outcome, causes }));
    }
    let causes: Vec<Cause> = [(left, Edge::End), (right, Edge::Start)]
        .into_iter()
        .filter_map(|(n, e)| n.map(|n| (n, e)))
        .filter_map(|(n, e)| cause(ctx, &n, false, e))
        .collect();
    let at = snap(t, shifted.unwrap_or(written), ctx.item);
    let outcome = match ctx.when {
        WhenLost::Snap => Outcome::Relocated,
        WhenLost::Refuse => Outcome::Conflict,
    };
    Some((at, Pending { outcome, causes }))
}

/// `NEIGHBOURS_GONE`: blamed on the record that removed the left neighbour (else the right),
/// with the placement in `details`.
fn refused(tx_id: &str, k: usize, item: &str, causes: Vec<Cause>) -> Conflict {
    let against = causes.first().map(|c| c.tx_id.clone());
    let placement = Placement {
        tx_id: tx_id.to_string(),
        op_index: k,
        item: item.to_string(),
        outcome: Outcome::Conflict,
        landed: None,
        causes,
    };
    let mut c = Conflict::new(tx_id, k, reasons::NEIGHBOURS_GONE, against);
    c.details = Some(Box::new(json!({ "placement": placement })));
    c
}

/// The last unmirrored record that removed or moved neighbour `name` (`present`: it is on the
/// track at the head): a removal, a touch of it as a whole, of its `start`, or of `edge` (the
/// edge that faces the item: A's end, C's start). `None` when no record did.
fn cause(ctx: &Ctx<'_>, name: &str, present: bool, edge: Edge) -> Option<Cause> {
    let id = real(name, ctx.bound);
    let start = Touch::Param("start".into());
    let changed = |f: &Footprint| {
        (!present && f.removes.iter().any(|x| x == id))
            || f.touches.iter().any(|(x, t)| {
                x == id && (*t == Touch::Exists || *t == Touch::Edge(edge) || *t == start)
            })
    };
    let (tx, _) = ctx.theirs.iter().rev().find(|(_, f)| changed(f))?;
    Some(Cause {
        neighbour: id.to_string(),
        change: if present {
            Change::Moved
        } else {
            Change::Removed
        },
        tx_id: tx.to_string(),
        author: None,
    })
}

/// The boundary of `t` nearest to `p` (an item's start or end, or the track's start), the
/// earlier on a tie; the item `own` is left out.
fn snap(t: &Track, p: Rational, own: &str) -> Rational {
    let mut best = Rational::zero();
    let dist = |b: Rational, best: &mut Rational| {
        let d = |x: Rational| if x >= p { x - p } else { p - x }.ok();
        match (d(b), d(*best)) {
            (Some(db), Some(dc)) if db < dc || db == dc && b < *best => *best = b,
            _ => {}
        }
    };
    for i in t.items.iter().filter(|i| i.id != own) {
        dist(i.start, &mut best);
        if let Ok(e) = i.end() {
            dist(e, &mut best);
        }
    }
    best
}

/// Where `item` is in `project`: its track and start, the frame of its timeline, and the
/// items that end where it starts and start where it ends.
fn landed(project: &Project, item: &str) -> Option<Landed> {
    for tl in &project.timelines {
        for t in &tl.tracks {
            let Some(i) = t.items.iter().find(|i| i.id == item) else {
                continue;
            };
            let end = i.end().ok();
            let others = || t.items.iter().filter(|o| o.id != item);
            let after = others().find(|o| o.end().ok() == Some(i.start)).map(id);
            let before = others().find(|o| Some(o.start) == end).map(id);
            let frame = i
                .start
                .checked_mul(tl.rate)
                .ok()
                .filter(|f| f.den() == 1)
                .map(|f| f.num());
            return Some(Landed {
                track: t.id.clone(),
                at: i.start,
                frame,
                after,
                before,
            });
        }
    }
    None
}

fn id(i: &Item) -> String {
    i.id.clone()
}
