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

/// What one step (or one part of a cut step) does, as [`steps_with_units`] reads it from the
/// links: it applies the record `base` again (`inverted: false`; the record itself, a redo of
/// its undo, an undo of its revert) or takes it back (`inverted: true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepUnit {
    pub base: String,
    pub inverted: bool,
}

/// `since` (in rev order) as steps, with the mirrors their links give (Decision 24):
///
/// - an `undo`, `redo` or `revert` addresses earlier steps: an undo its units (a unit is a
///   record's steps, or a merge group's records' steps, by `mergeId`), a redo its undo, a
///   revert its group. When they are all here and the op counts add up (each op inverts to
///   one op), it is cut into one part per addressed step, in the order its ops invert them
///   (newest first), and each part mirrors its step when that step is not mirrored yet.
///   A step already mirrored (an undo of a revert: the revert is paired with the group) leaves
///   the part unmirrored: it applies the step's record again, and a later inverse of it pairs
///   with that part ([`StepUnit`] says which).
///
/// Anything else stands alone: an undo or revert whose units are not all here (a merge group
/// only partly here, say), or whose op counts do not add up, and every other kind.
/// Fails only as [`Step::from_committed`] does.
pub fn steps_from(since: &[&CommittedTx]) -> Result<Vec<Step>, CoreError> {
    Ok(steps_with_units(since)?.0)
}

/// [`steps_from`], with each step's [`StepUnit`] (`None` for a step that stands alone
/// without being a record of its own: an undo, redo or revert whose units are not here).
pub fn steps_with_units(
    since: &[&CommittedTx],
) -> Result<(Vec<Step>, Vec<Option<StepUnit>>), CoreError> {
    let mut steps: Vec<Step> = Vec::new();
    let mut units: Vec<Option<StepUnit>> = Vec::new();
    let mut paired: Vec<bool> = Vec::new();
    let mut by_tx: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    // Merge groups by `mergeId`: their records' steps, oldest first.
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for c in since {
        let step = Step::from_committed(c)?;
        // The steps a unit's inverse addresses, in the order its ops invert them: a record's
        // steps (parts) newest first, a merge group's records' steps newest first.
        let unit = |id: &str| -> Option<Vec<usize>> {
            by_tx
                .get(id)
                .or_else(|| groups.get(id))
                .map(|g| g.iter().rev().copied().collect())
        };
        let addressed: Option<Vec<usize>> = match c.link.as_deref() {
            Some(Link::Undo { undoes }) => undoes
                .iter()
                .map(|u| unit(u))
                .collect::<Option<Vec<_>>>()
                .map(|t| t.concat()),
            Some(Link::Redo { redoes }) => unit(redoes),
            Some(Link::Revert { reverts }) => groups
                .get(reverts)
                .map(|g| g.iter().rev().copied().collect()),
            _ => None,
        };
        let addressed = addressed.filter(|t| {
            let distinct = t.iter().collect::<BTreeSet<_>>().len() == t.len();
            let ops: usize = t.iter().map(|&j| steps[j].ops.len()).sum();
            distinct && !t.is_empty() && ops == step.ops.len()
        });
        let start = steps.len();
        match addressed {
            Some(t) => {
                let sizes: Vec<usize> = t.iter().map(|&j| steps[j].ops.len()).collect();
                for (mut part, j) in step.split(&sizes)?.into_iter().zip(t) {
                    if !paired[j] {
                        part.mirror_of = Some(j);
                        paired[j] = true;
                    }
                    let unit = units[j].as_ref().map(|u| StepUnit {
                        base: u.base.clone(),
                        inverted: !u.inverted,
                    });
                    paired.push(part.mirror_of.is_some());
                    steps.push(part);
                    units.push(unit);
                }
            }
            None => {
                let own = match c.link.as_deref() {
                    Some(Link::Undo { .. } | Link::Redo { .. } | Link::Revert { .. }) => None,
                    _ => Some(StepUnit {
                        base: c.tx.tx_id.clone(),
                        inverted: false,
                    }),
                };
                steps.push(step);
                paired.push(false);
                units.push(own);
            }
        }
        by_tx
            .entry(c.tx.tx_id.clone())
            .or_default()
            .extend(start..steps.len());
        if let Some(Link::Merge { merge_id, .. }) = c.link.as_deref() {
            groups
                .entry(merge_id.clone())
                .or_default()
                .extend(start..steps.len());
        }
    }
    Ok((steps, units))
}
