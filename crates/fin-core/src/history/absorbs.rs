//! Which removals of a planned transaction put their item's content back into another item
//! (spec §4.3, *as built*): the lift `item.remove` of a split's right part, followed by the
//! strict trim that grows the left part back over it. That is the inverse of `item.split` and
//! `caption.split`. The engine judges such a removal as a put-back of the removed item
//! (`Rebase::absorbs`). Another author's later edit of the right part would be lost inside the
//! left part, so "removing an entity someone else edited is not a conflict" does not hold.
//!
//! The marks come from the records' links, never from the ops' shape: the undo of an overwrite
//! insert inverts to the same two ops (`util::carve`), and it removes an entity of its own,
//! which is no conflict. Two marks per position, in a record's ops ([`in_ops`]) and in its
//! stored inverse ([`in_inverse`]):
//! - `absorb`: a lift removal that the next op's growing strict trim takes back;
//! - `reverse`: the inverse of such a pair, a shrinking strict trim followed by a lift restore
//!   (a redo of a split's undo), whose own inverse absorbs again.
//!
//! An `edit` has no marks in its ops (clients send neither op). Its inverse absorbs at each
//! split's part. An `undo` or `revert` holds its units' inverses, newest record first, one op
//! per op; a `redo` holds its undo's inverse; a `merge` or `cherryPick` copy holds its origin
//! root's ops. A record's inverse lists each op's inverse in reverse op order, so a marked pair
//! `(j, j + 1)` in its ops inverts to the pair starting at op `j + 1`'s part, the other mark.
//! Every mark is checked against the ops at its position; a record whose inverse does not
//! have the expected length gets none.

use std::collections::{BTreeSet, HashMap};

use super::{LogRec, LogView};
use crate::error::CoreError;
use crate::ops::{self, DeleteMode, Edge, Op, TrimMode};
use crate::rebase::committed_ops;
use crate::record::Link;

/// The marked positions of one list of ops (module docs).
#[derive(Debug, Clone, Default)]
struct Marks {
    absorb: BTreeSet<usize>,
    reverse: BTreeSet<usize>,
}

impl Marks {
    fn extend(&mut self, other: &Marks, offset: usize) {
        self.absorb.extend(other.absorb.iter().map(|k| k + offset));
        self.reverse
            .extend(other.reverse.iter().map(|k| k + offset));
    }
}

type Memo = HashMap<(String, bool), Marks>;

/// The split-undo removals in the inverses of `recs`, laid end to end in that order (as an
/// undo, a redo or a revert takes them).
pub(super) fn of_inverses(
    log: &dyn LogView,
    recs: &[&LogRec],
) -> Result<BTreeSet<usize>, CoreError> {
    let mut memo = Memo::new();
    let mut out = Marks::default();
    let mut offset = 0;
    for rec in recs {
        out.extend(&in_inverse(log, rec, &mut memo)?, offset);
        offset += rec.record.inverse.len();
    }
    Ok(out.absorb)
}

/// The split-undo removals in the ops of `rec` (for a copy of it, which keeps its ops).
pub(super) fn of_ops(log: &dyn LogView, rec: &LogRec) -> Result<BTreeSet<usize>, CoreError> {
    Ok(in_ops(log, rec, &mut Memo::new())?.absorb)
}

/// The marks in the ops of `rec`.
fn in_ops(log: &dyn LogView, rec: &LogRec, memo: &mut Memo) -> Result<Marks, CoreError> {
    let key = (rec.record.tx_id.clone(), false);
    if let Some(m) = memo.get(&key) {
        return Ok(m.clone());
    }
    let link = rec.committed()?.link.map(|l| *l);
    let sources: Vec<LogRec> = match &link {
        Some(Link::Undo { undoes }) => {
            let mut recs = Vec::new();
            for unit in undoes {
                recs.extend(unit_records(log, unit)?);
            }
            recs
        }
        Some(Link::Redo { redoes }) => log.find(redoes)?.into_iter().collect(),
        Some(Link::Revert { reverts }) => newest_first(log.merge_group(reverts)?),
        Some(Link::Merge { origin, .. } | Link::CherryPick { origin, .. }) => {
            let marks = match log.find(origin)? {
                Some(o) if o.record.tx_id != rec.record.tx_id => in_ops(log, &o, memo)?,
                _ => Marks::default(),
            };
            memo.insert(key, marks.clone());
            return Ok(marks);
        }
        _ => Vec::new(),
    };
    let mut marks = Marks::default();
    let mut offset = 0;
    for s in &sources {
        marks.extend(&in_inverse(log, s, memo)?, offset);
        offset += s.record.inverse.len();
    }
    if offset != rec.record.intent.ops.len() {
        marks = Marks::default();
    }
    memo.insert(key, marks.clone());
    Ok(marks)
}

/// The records a unit id names, newest first: a record, or a merge group's records.
fn unit_records(log: &dyn LogView, unit: &str) -> Result<Vec<LogRec>, CoreError> {
    Ok(match log.find(unit)? {
        Some(rec) => vec![rec],
        None => newest_first(log.merge_group(unit)?),
    })
}

fn newest_first(mut recs: Vec<LogRec>) -> Vec<LogRec> {
    recs.sort_by_key(|r| std::cmp::Reverse(r.seq));
    recs
}

/// The marks in the stored inverse of `rec`.
fn in_inverse(log: &dyn LogView, rec: &LogRec, memo: &mut Memo) -> Result<Marks, CoreError> {
    let key = (rec.record.tx_id.clone(), true);
    if let Some(m) = memo.get(&key) {
        return Ok(m.clone());
    }
    let ops = committed_ops(&rec.committed()?)?;
    let inverse = &rec.record.inverse;
    let lens: Vec<usize> = ops.iter().map(|(op, d)| ops::inverse_len(op, d)).collect();
    let mut marks = Marks::default();
    if lens.iter().sum::<usize>() == inverse.len() {
        // Op `j`'s part starts after the parts of every later op.
        let start = |j: usize| lens[j + 1..].iter().sum::<usize>();
        for (j, (op, _)) in ops.iter().enumerate() {
            if matches!(op, Op::ItemSplit(_) | Op::CaptionSplit(_)) {
                let part = start(j)..start(j) + lens[j];
                marks.absorb.extend(part.filter(|&k| absorbing(inverse, k)));
            }
        }
        let own = in_ops(log, rec, memo)?;
        for &j in &own.absorb {
            if j + 1 < ops.len() && reversing(inverse, start(j + 1)) {
                marks.reverse.insert(start(j + 1));
            }
        }
        for &j in &own.reverse {
            if j + 1 < ops.len() && absorbing(inverse, start(j + 1)) {
                marks.absorb.insert(start(j + 1));
            }
        }
    }
    memo.insert(key, marks.clone());
    Ok(marks)
}

/// A lift removal at `k` followed by a strict trim that grows an end.
fn absorbing(ops: &[Op], k: usize) -> bool {
    matches!(
        (ops.get(k), ops.get(k + 1)),
        (Some(Op::ItemRemove(r)), Some(Op::ItemTrim(t)))
            if r.mode == DeleteMode::Lift
                && t.mode == TrimMode::Strict
                && t.edge == Edge::End
                && t.delta.is_positive()
    )
}

/// A strict trim that shrinks an end at `k` followed by a lift restore.
fn reversing(ops: &[Op], k: usize) -> bool {
    matches!(
        (ops.get(k), ops.get(k + 1)),
        (Some(Op::ItemTrim(t)), Some(Op::ItemRestore(r)))
            if r.mode == DeleteMode::Lift
                && t.mode == TrimMode::Strict
                && t.edge == Edge::End
                && t.delta.is_negative()
    )
}
