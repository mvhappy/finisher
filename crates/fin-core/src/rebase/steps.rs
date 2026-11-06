//! Intervening records as engine steps, with their mirrors wired from the records' links
//! (Decision 24).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::error::CoreError;
use crate::ops::Op;
use crate::record::{Link, resolve_ops};
use crate::tx::{CommittedTx, Diff};

use super::footprint::{Footprint, footprint, join_diffs, split_diff};
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

    /// The step cut into consecutive parts of `sizes` ops each; each part takes its ops' own
    /// diffs, renumbered from 0. `sizes` must add up to the op count.
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
                    diff: join_diffs(&diffs[range]),
                    mirror_of: None,
                    invert: false,
                }
            })
            .collect())
    }
}

/// A step as the engine reads it ([`super::rebase_prepared`]): its map and its ops' footprints,
/// worked out once. A clone shares them, so a merge hands the same steps to every candidate
/// without copying records (plan 4B Task 20). Errors are kept and raised where the engine
/// would meet them: a footprint only for a step in no mirrored pair.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub tx_id: Arc<str>,
    pub mirror_of: Option<usize>,
    pub(super) map: Result<StepMap, CoreError>,
    pub(super) footprints: Result<Arc<[Footprint]>, CoreError>,
}

impl Prepared {
    pub fn new(step: &Step) -> Prepared {
        Prepared {
            tx_id: step.tx_id.as_str().into(),
            mirror_of: step.mirror_of,
            map: step.map(),
            footprints: step.footprints().map(Into::into),
        }
    }
}

/// A step that may name an earlier one it mirrors ([`Step`], [`Prepared`]).
pub(crate) trait Mirrored: Clone {
    fn mirror_of(&self) -> Option<usize>;
    fn set_mirror_of(&mut self, m: Option<usize>);
}

impl Mirrored for Step {
    fn mirror_of(&self) -> Option<usize> {
        self.mirror_of
    }
    fn set_mirror_of(&mut self, m: Option<usize>) {
        self.mirror_of = m;
    }
}

impl Mirrored for Prepared {
    fn mirror_of(&self) -> Option<usize> {
        self.mirror_of
    }
    fn set_mirror_of(&mut self, m: Option<usize>) {
        self.mirror_of = m;
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
/// An undo whose units are only partly here (a stale base between two units of an
/// `undo --to`) names them newest first, so the units here come first, and their inverses
/// lead its ops: it is cut into one part per step of theirs, as above, and one unpaired tail
/// part for the rest (the inverses of units older than the interval, which stand alone).
///
/// Anything else stands alone: an undo whose units here are not a prefix of the ones it
/// names, a revert whose group is not all here, a step whose op counts do not add up, and
/// every other kind.
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
        // The addressed steps, and whether some named units are not here (`partial`).
        let addressed: Option<(Vec<usize>, bool)> = match c.link.as_deref() {
            Some(Link::Undo { undoes }) => {
                let here: Vec<Option<Vec<usize>>> = undoes.iter().map(|u| unit(u)).collect();
                let n = here.iter().take_while(|u| u.is_some()).count();
                // The units here must be a prefix of the named ones (newest first).
                (here[n..].iter().all(Option::is_none)).then(|| {
                    (
                        here[..n].iter().flatten().flatten().copied().collect(),
                        n < here.len(),
                    )
                })
            }
            Some(Link::Redo { redoes }) => unit(redoes).map(|t| (t, false)),
            Some(Link::Revert { reverts }) => groups
                .get(reverts)
                .map(|g| (g.iter().rev().copied().collect(), false)),
            _ => None,
        };
        let addressed = addressed.filter(|(t, partial)| {
            let distinct = t.iter().collect::<BTreeSet<_>>().len() == t.len();
            let ops: usize = t.iter().map(|&j| steps[j].ops.len()).sum();
            let fits = if *partial {
                ops < step.ops.len()
            } else {
                ops == step.ops.len()
            };
            distinct && !t.is_empty() && fits
        });
        let start = steps.len();
        match addressed {
            Some((t, partial)) => {
                let mut sizes: Vec<usize> = t.iter().map(|&j| steps[j].ops.len()).collect();
                let tail = step.ops.len() - sizes.iter().sum::<usize>();
                if partial {
                    sizes.push(tail);
                }
                let mut parts = step.split(&sizes)?;
                // The inverses of units older than the interval: one unpaired part.
                let rest = partial.then(|| parts.pop()).flatten();
                for (mut part, j) in parts.into_iter().zip(t) {
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
                if let Some(part) = rest {
                    paired.push(false);
                    steps.push(part);
                    units.push(None);
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
