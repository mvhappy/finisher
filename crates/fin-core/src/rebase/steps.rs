//! Intervening records as engine steps, with their mirrors wired from the records' links
//! (Decision 24).

use std::collections::{BTreeMap, BTreeSet};

use crate::error::CoreError;
use crate::ops::Op;
use crate::record::{Link, resolve_ops};
use crate::tx::{CommittedTx, Diff};

use super::footprint::{Footprint, footprint, split_diff};
use super::map::StepMap;

/// One intervening record (or one unit's part of an `undo` record), as the engine sees it: its
/// ops (placeholders resolved) and diff.
///
/// - `invert`: the step stands for the record undone (its map is the inverse of `diff`'s, and
///   its footprint swaps `creates` and `removes`).
/// - `mirror_of: Some(j)`: this step exactly inverts step `j < self` (an undo of it, a copy of
///   its inverted original, Decision 24). Positions deleted by one are recovered through the
///   other, and neither is judged for entity conflicts.
#[derive(Debug, Clone)]
pub struct Step {
    pub tx_id: String,
    pub ops: Vec<Op>,
    pub diff: Diff,
    pub mirror_of: Option<usize>,
    pub invert: bool,
}

impl Step {
    /// A committed record as a forward, unmirrored step. Fails when its placeholders have no
    /// recorded binding (`UNBOUND_PLACEHOLDER`).
    pub fn from_committed(c: &CommittedTx) -> Result<Step, CoreError> {
        Ok(Step {
            tx_id: c.tx.tx_id.clone(),
            ops: resolve_ops(&c.tx.ops, &c.bindings)?,
            diff: c.diff.clone(),
            mirror_of: None,
            invert: false,
        })
    }

    pub(super) fn map(&self) -> Result<StepMap, CoreError> {
        let map = StepMap::from_diff(&self.diff);
        if self.invert { map.invert() } else { Ok(map) }
    }

    pub(super) fn footprints(&self) -> Result<Vec<Footprint>, CoreError> {
        let diffs = split_diff(&self.ops, &self.diff)?;
        let mut out: Vec<Footprint> = self
            .ops
            .iter()
            .zip(&diffs)
            .map(|(op, d)| footprint(op, d))
            .collect();
        if self.invert {
            out.reverse();
            for f in &mut out {
                std::mem::swap(&mut f.creates, &mut f.removes);
            }
        }
        Ok(out)
    }

    /// The step cut into consecutive parts of `sizes` ops each (each op records at most one
    /// shift, so each part takes its ops' shifts). `sizes` must add up to the op count.
    fn split(&self, sizes: &[usize]) -> Result<Vec<Step>, CoreError> {
        let diffs = split_diff(&self.ops, &self.diff)?;
        let mut at = 0;
        Ok(sizes
            .iter()
            .map(|&n| {
                let range = at..at + n;
                at += n;
                Step {
                    tx_id: self.tx_id.clone(),
                    ops: self.ops[range.clone()].to_vec(),
                    diff: Diff {
                        changes: Vec::new(),
                        shifts: diffs[range].iter().flat_map(|d| d.shifts.clone()).collect(),
                    },
                    mirror_of: None,
                    invert: false,
                }
            })
            .collect())
    }
}

/// `since` (in rev order) as steps, with the mirrors their links give (Decision 24):
///
/// - an `undo` whose units are each one earlier step, not yet mirrored, is cut into one part
///   per unit (its ops are the units' inverses, newest unit first, and every op inverts to one
///   op), and each part mirrors its unit;
///   a unit that is a merge group (its `mergeId`) is its `merge` records here, each one step
///   not yet mirrored, newest first (an undo takes a group's inverses newest record first);
/// - a `redo` of an `undo` that is one earlier step, not yet mirrored, mirrors it (the undo's
///   unit was before `since`; with it inside, the undo is paired with the unit already).
///
/// Anything else stands alone: an undo whose units are not all here (or whose op counts do not
/// add up: a merge group only partly here, say), a `revert` (Task 14), and every other kind.
/// Fails only as [`Step::from_committed`] does.
pub fn steps_from(since: &[&CommittedTx]) -> Result<Vec<Step>, CoreError> {
    let mut steps: Vec<Step> = Vec::new();
    let mut paired: Vec<bool> = Vec::new();
    let mut by_tx: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    // Merge groups by `mergeId`: their records' steps, oldest first.
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for c in since {
        let step = Step::from_committed(c)?;
        let one = |id: &str| match by_tx.get(id).map(Vec::as_slice) {
            Some(&[j]) if !paired[j] => Some(j),
            _ => None,
        };
        // A unit's steps, in the order its inverses stand in an undo.
        let unit = |id: &str| match (one(id), groups.get(id)) {
            (Some(j), _) => Some(vec![j]),
            (None, Some(g)) if g.iter().all(|&j| !paired[j]) => {
                Some(g.iter().rev().copied().collect())
            }
            _ => None,
        };
        let targets: Option<Vec<usize>> = match c.link.as_deref() {
            Some(Link::Undo { undoes }) => undoes
                .iter()
                .map(|u| unit(u))
                .collect::<Option<Vec<_>>>()
                .map(|t| t.concat()),
            Some(Link::Redo { redoes }) => one(redoes).map(|j| vec![j]),
            _ => None,
        };
        let targets = targets.filter(|t| {
            let distinct = t.iter().collect::<BTreeSet<_>>().len() == t.len();
            let ops: usize = t.iter().map(|&j| steps[j].ops.len()).sum();
            distinct && !t.is_empty() && ops == step.ops.len()
        });
        let start = steps.len();
        match targets {
            Some(t) => {
                let sizes: Vec<usize> = t.iter().map(|&j| steps[j].ops.len()).collect();
                for (mut part, j) in step.split(&sizes)?.into_iter().zip(t) {
                    part.mirror_of = Some(j);
                    paired[j] = true;
                    steps.push(part);
                    paired.push(true);
                }
            }
            None => {
                steps.push(step);
                paired.push(false);
            }
        }
        by_tx
            .entry(c.tx.tx_id.clone())
            .or_default()
            .extend(start..steps.len());
        if let Some(Link::Merge { merge_id, .. }) = c.link.as_deref() {
            groups.entry(merge_id.clone()).or_default().push(start);
        }
    }
    Ok(steps)
}
