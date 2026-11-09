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

use std::collections::{BTreeSet, HashMap, HashSet};

use super::origin::chain_error;
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

/// `(txId, inverse?)` → the marks of that record's ops (`false`) or stored inverse (`true`).
type Key = (String, bool);
type Memo = HashMap<Key, Marks>;

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
        out.extend(&marks(log, rec, true, &mut memo)?, offset);
        offset += rec.record.inverse.len();
    }
    Ok(out.absorb)
}

/// The split-undo removals in the ops of `rec` (for a copy of it, which keeps its ops).
pub(super) fn of_ops(log: &dyn LogView, rec: &LogRec) -> Result<BTreeSet<usize>, CoreError> {
    Ok(marks(log, rec, false, &mut Memo::new())?.absorb)
}

/// What one step of [`marks`] found for a record.
enum Step {
    Done(Marks),
    /// The marks of these records are needed first (none of them in `memo` yet).
    Needs(Vec<(LogRec, bool)>),
}

/// The marks in the ops (`inverse == false`) or the stored inverse of `rec`. A record's marks
/// depend on earlier records' (an undo's on its units' inverses, a redo's on its undo's, a
/// copy's on its origin's), so a chain of undo/redo cycles is as deep as the history: the
/// records are evaluated from an explicit stack, deepest first, each once (`memo`), never by
/// recursion (review of 2026-11-09, #15). A record needed while it is being evaluated (a loop
/// of stored links: an undo of itself, a copy of a copy of itself) is `DB_ERROR` `linkCycle`.
fn marks(
    log: &dyn LogView,
    rec: &LogRec,
    inverse: bool,
    memo: &mut Memo,
) -> Result<Marks, CoreError> {
    let top = (rec.record.tx_id.clone(), inverse);
    let mut stack: Vec<(LogRec, bool)> = vec![(rec.clone(), inverse)];
    let mut open: HashSet<Key> = HashSet::new();
    while let Some((r, inv)) = stack.last() {
        let key = (r.record.tx_id.clone(), *inv);
        if memo.contains_key(&key) {
            stack.pop();
            continue;
        }
        let step = if *inv {
            in_inverse(r, memo)?
        } else {
            in_ops(log, r, memo)?
        };
        match step {
            Step::Done(m) => {
                open.remove(&key);
                memo.insert(key, m);
                stack.pop();
            }
            Step::Needs(deps) => {
                for (d, dinv) in deps {
                    if open.contains(&(d.record.tx_id.clone(), dinv)) {
                        return Err(chain_error("linkCycle", &rec.record.tx_id, &d.record.tx_id));
                    }
                    stack.push((d, dinv));
                }
                open.insert(key);
            }
        }
    }
    Ok(memo.get(&top).cloned().unwrap_or_default())
}

/// The records of `wanted` that `memo` has no marks for, as [`Step::Needs`], or `None`.
fn needs<'r>(memo: &Memo, wanted: impl IntoIterator<Item = (&'r LogRec, bool)>) -> Option<Step> {
    let missing: Vec<(LogRec, bool)> = wanted
        .into_iter()
        .filter(|(r, inv)| !memo.contains_key(&(r.record.tx_id.clone(), *inv)))
        .map(|(r, inv)| (r.clone(), inv))
        .collect();
    (!missing.is_empty()).then_some(Step::Needs(missing))
}

fn got(memo: &Memo, rec: &LogRec, inverse: bool) -> Marks {
    memo.get(&(rec.record.tx_id.clone(), inverse))
        .cloned()
        .unwrap_or_default()
}

/// The marks in the ops of `rec`, once its sources' are in `memo`.
fn in_ops(log: &dyn LogView, rec: &LogRec, memo: &Memo) -> Result<Step, CoreError> {
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
            return Ok(match log.find(origin)? {
                Some(o) if o.record.tx_id == rec.record.tx_id => {
                    return Err(chain_error("originCycle", origin, origin));
                }
                Some(o) => {
                    needs(memo, [(&o, false)]).unwrap_or_else(|| Step::Done(got(memo, &o, false)))
                }
                None => Step::Done(Marks::default()),
            });
        }
        _ => Vec::new(),
    };
    if let Some(step) = needs(memo, sources.iter().map(|s| (s, true))) {
        return Ok(step);
    }
    let mut marks = Marks::default();
    let mut offset = 0;
    for s in &sources {
        marks.extend(&got(memo, s, true), offset);
        offset += s.record.inverse.len();
    }
    if offset != rec.record.intent.ops.len() {
        marks = Marks::default();
    }
    Ok(Step::Done(marks))
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

/// The marks in the stored inverse of `rec`, once the marks of its own ops are in `memo`.
fn in_inverse(rec: &LogRec, memo: &Memo) -> Result<Step, CoreError> {
    let ops = committed_ops(&rec.committed()?)?;
    let inverse = &rec.record.inverse;
    let lens: Vec<usize> = ops.iter().map(|(op, d)| ops::inverse_len(op, d)).collect();
    let mut marks = Marks::default();
    if lens.iter().sum::<usize>() == inverse.len() {
        if let Some(step) = needs(memo, [(rec, false)]) {
            return Ok(step);
        }
        // Op `j`'s part starts after the parts of every later op.
        let start = |j: usize| lens[j + 1..].iter().sum::<usize>();
        for (j, (op, _)) in ops.iter().enumerate() {
            if matches!(op, Op::ItemSplit(_) | Op::CaptionSplit(_)) {
                let part = start(j)..start(j) + lens[j];
                marks.absorb.extend(part.filter(|&k| absorbing(inverse, k)));
            }
        }
        let own = got(memo, rec, false);
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
    Ok(Step::Done(marks))
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
