//! Merge (spec §4.4; plan 4B Decisions 18–20, 24): the source's candidates
//! ([`candidates`](super::candidates)) re-run, in order, at the target's head, each through the
//! rebase engine with full policy.
//!
//! - **Common point.** The two lineages share a branch, nearest first; the common point C is
//!   the lower of the two seqs up to which each lineage reads it ([`common_point`]). A path is
//!   a lineage's records after C, oldest first: on C's branch above C's seq, then on each
//!   branch below it in the lineage.
//! - **Steps.** A candidate's own diff is its stored diff, in its source frame (the source
//!   state right before it). Its steps lead from that frame to the target head
//!   ([`candidate_steps`]): the source path before it inverted, newest first; the target path
//!   (with its undo/redo mirrors, [`steps_from`]); the copies this merge planned so far. Each
//!   copy mirrors its inverted original, and so does a target `merge` or `cherryPick` record
//!   of a source record (or the target record a source copy came from), so positions inside
//!   an original come back through its copy (Decision 24). An original with no copy (skipped,
//!   excluded) stays an unmirrored inverse: what depends on it conflicts.
//! - **Copies** keep the source's entity ids: the re-run binds the source record's bindings
//!   ([`BoundIds`]), in `Client` mode for `edit`/`cherryPick`, `Internal` for
//!   `undo`/`redo`/`revert` (inverses may remove and restore). A `DUPLICATE_ID` there is
//!   `ALREADY_PRESENT` when a target record of the same origin chain bound the id, else
//!   `ID_COLLISION` (Decisions 18, 33). The copy is a `merge` record `<mergeId>-<n>` with
//!   `origin` = the source txId; its intent and record author are the merging author
//!   (Decision 20), its bindings the source's.
//! - **Conflicts.** A failing candidate is recorded and treated as skipped; planning goes on,
//!   so the full list comes back. A conflict blamed on a skipped candidate's inverse (its
//!   insert gone, say) is `DEPENDS_ON_SKIPPED`. Without `skip_conflicting`, any conflict makes
//!   the plan `CONFLICT` with every conflict in `details.conflicts`.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::json;

use super::candidates::candidates;
use super::origin::{root_of, target_roots};
use super::{Lineage, LogRec, LogView, Planned};
use crate::apply::ApplyMode;
use crate::error::CoreError;
use crate::rebase::{Conflict, Conflicts, Rebase, Step, reasons, rebase_apply, steps_from};
use crate::record::{Link, RecordKind};
use crate::replay::BoundIds;
use crate::tx::{CommittedTx, Tx};

/// A `through` past any head (`records` reads to the head): seqs are SQLite integers.
pub(super) const TO_HEAD: u64 = i64::MAX as u64;

/// The common point of two lineages: the first branch of `a` (own first) that `b` also reads,
/// and the lower of the seqs up to which each reads it. `("", 0)` when they share no branch
/// (two roots). Two lineages of the same branch give `u64::MAX`.
pub fn common_point(a: &Lineage, b: &Lineage) -> (String, u64) {
    for (branch, _, ta) in &a.segs {
        if let Some((_, _, tb)) = b.segs.iter().find(|s| &s.0 == branch) {
            let seq = match (ta, tb) {
                (Some(x), Some(y)) => (*x).min(*y),
                (Some(x), None) | (None, Some(x)) => *x,
                (None, None) => u64::MAX,
            };
            return (branch.clone(), seq);
        }
    }
    (String::new(), 0)
}

/// `l`'s records after the common point `c` (on `l`'s lineage), oldest first.
pub(super) fn path_after(
    log: &dyn LogView,
    l: &Lineage,
    c: &(String, u64),
) -> Result<Vec<LogRec>, CoreError> {
    let Some(at) = l.segs.iter().position(|s| s.0 == c.0) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (branch, after, through) in l.segs[..=at].iter().rev() {
        let after = if *branch == c.0 {
            (*after).max(c.1)
        } else {
            *after
        };
        let through = through.unwrap_or(TO_HEAD);
        if through > after {
            out.extend(log.records(branch, after, through)?);
        }
    }
    Ok(out)
}

/// The source path inverted and the target path as steps, computed once per merge; each
/// candidate takes its slice ([`Paths::steps`]).
pub(super) struct Paths {
    /// `inv[j]`: source record `j` inverted.
    inv: Vec<Step>,
    /// The target path's steps, mirrors relative to this list.
    target: Vec<Step>,
    /// `(target step, source index)`: a target record and a source record that copy one
    /// another (either way).
    cross: Vec<(usize, usize)>,
}

impl Paths {
    pub(super) fn new(source: &[LogRec], target: &[LogRec]) -> Result<Paths, CoreError> {
        let inv = source
            .iter()
            .map(|r| {
                let mut s = Step::from_committed(&r.committed()?)?;
                s.invert = true;
                Ok(s)
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        let committed: Vec<CommittedTx> = target
            .iter()
            .map(LogRec::committed)
            .collect::<Result<_, _>>()?;
        let target_steps = steps_from(&committed.iter().collect::<Vec<_>>())?;
        let mut by_tx: HashMap<&str, Vec<usize>> = HashMap::new();
        for (q, s) in target_steps.iter().enumerate() {
            by_tx.entry(s.tx_id.as_str()).or_default().push(q);
        }
        let src: HashMap<&str, usize> = source
            .iter()
            .enumerate()
            .map(|(j, r)| (r.record.tx_id.as_str(), j))
            .collect();
        let copy_of = |r: &LogRec| match r.record.kind {
            RecordKind::Merge | RecordKind::CherryPick => r.record.origin.clone(),
            _ => None,
        };
        let mut cross = Vec::new();
        let mut used = HashSet::new();
        for t in target {
            let Some(&[q]) = by_tx.get(t.record.tx_id.as_str()).map(Vec::as_slice) else {
                continue;
            };
            let j = copy_of(t)
                .and_then(|o| src.get(o.as_str()).copied())
                .or_else(|| {
                    source
                        .iter()
                        .position(|s| copy_of(s).as_deref() == Some(t.record.tx_id.as_str()))
                });
            if let Some(j) = j
                && used.insert(j)
            {
                cross.push((q, j));
            }
        }
        Ok(Paths {
            inv,
            target: target_steps,
            cross,
        })
    }

    /// The steps of source candidate `i`, with `planned` (source index, copy step) so far.
    pub(super) fn steps(&self, i: usize, planned: &[(usize, Step)]) -> Vec<Step> {
        let at = |j: usize| i - 1 - j;
        let mut steps: Vec<Step> = (0..i).rev().map(|j| self.inv[j].clone()).collect();
        let mut paired = vec![false; i + self.target.len() + planned.len()];
        for t in &self.target {
            let mut t = t.clone();
            t.mirror_of = t.mirror_of.map(|m| m + i);
            if let Some(m) = t.mirror_of {
                paired[m] = true;
                paired[steps.len()] = true;
            }
            steps.push(t);
        }
        for &(q, j) in &self.cross {
            let (a, b) = (j, i + q);
            if j < i && !paired[at(a)] && !paired[b] {
                steps[b].mirror_of = Some(at(a));
                paired[at(a)] = true;
                paired[b] = true;
            }
        }
        for (j, step) in planned {
            let mut s = step.clone();
            if *j < i && !paired[at(*j)] {
                s.mirror_of = Some(at(*j));
                paired[at(*j)] = true;
                paired[steps.len()] = true;
            }
            steps.push(s);
        }
        steps
    }
}

/// The steps that lead from source candidate `i`'s frame to the target head (module docs):
/// the source path before `i` inverted, newest first; the target path; the copies `planned`
/// so far (source index, the copy as committed). Fails only as [`Step::from_committed`].
pub fn candidate_steps(
    source_path: &[LogRec],
    i: usize,
    target_path: &[LogRec],
    planned: &[(usize, CommittedTx)],
) -> Result<Vec<Step>, CoreError> {
    let paths = Paths::new(source_path, target_path)?;
    let planned = planned
        .iter()
        .map(|(j, c)| Ok((*j, Step::from_committed(c)?)))
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok(paths.steps(i, &planned))
}

/// A planned merge: its id, the copies to append in order, the source txIds they copy, the
/// candidates that conflict (each skipped; empty unless `skip_conflicting`), and the source
/// records left out as `UNDOES_TARGET_RECORDS`.
#[derive(Debug, Clone, PartialEq)]
pub struct MergePlan {
    pub merge_id: String,
    pub planned: Vec<Planned>,
    pub merged: Vec<String>,
    pub conflicts: Vec<Conflict>,
    pub excluded: Vec<Conflict>,
}

/// The `merge` records that merge branch `from` into branch `into` (ids), by `author`, as
/// merge group `merge_id` (module docs). Nothing to merge is an empty plan.
///
/// Errors: `INVALID_OP` when `from` is `into` or the two share no history; `CONFLICT`
/// (`details.conflicts`: every conflicting candidate, in order) unless `skip_conflicting`;
/// what the log's reads fail with.
pub fn plan_merge(
    log: &dyn LogView,
    from: &str,
    into: &str,
    author: &str,
    merge_id: &str,
    skip_conflicting: bool,
) -> Result<MergePlan, CoreError> {
    if from == into {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("branch {from} cannot be merged into itself"),
        )
        .with_details(json!({ "from": from, "into": into }))
        .with_fix("name another branch to merge into (--into)"));
    }
    let (source_l, target_l) = (log.lineage(from)?, log.lineage(into)?);
    let c = common_point(&source_l, &target_l);
    if c.0.is_empty() {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("branches {from} and {into} share no history"),
        )
        .with_details(json!({ "from": from, "into": into })));
    }
    let source = path_after(log, &source_l, &c)?;
    let target = path_after(log, &target_l, &c)?;
    let cands = candidates(&source, &target);
    let paths = Paths::new(&source, &target)?;
    let roots = target_roots(log, &target)?;
    let (mut seq, mut head) = log.head(into)?;

    let mut plan = MergePlan {
        merge_id: merge_id.to_string(),
        planned: Vec::new(),
        merged: Vec::new(),
        conflicts: Vec::new(),
        excluded: cands
            .excluded
            .iter()
            .map(|&(i, reason)| Conflict::new(&source[i].record.tx_id, 0, reason, None))
            .collect(),
    };
    let mut copies: Vec<(usize, Step)> = Vec::new();
    let mut skipped: HashSet<&str> = HashSet::new();
    for &i in &cands.picks {
        let src = &source[i];
        let r = &src.record;
        let steps = paths.steps(i, &copies);
        let root = root_of(log, src)?;
        let same_origin: BTreeSet<String> = roots
            .iter()
            .filter(|(t, _)| *t == root)
            .flat_map(|(_, ids)| ids.iter().cloned())
            .collect();
        let tx = Tx {
            tx_id: r.tx_id.clone(),
            ..r.intent.clone()
        };
        let mode = match r.kind {
            RecordKind::Edit | RecordKind::CherryPick => ApplyMode::Client,
            _ => ApplyMode::Internal,
        };
        let rebase = Rebase {
            tx: &tx,
            own: &src.diff,
            steps: &steps,
            same_origin: &same_origin,
        };
        let rerun = BoundIds::new(&r.intent.ops, &r.bindings)
            .map_err(|e| Conflict::from_rerun(&e, &r.tx_id, 0, None))
            .and_then(|mut ids| rebase_apply(&head, &rebase, &mut ids, mode));
        let rebased = match rerun {
            Ok(rebased) => rebased,
            Err(c) => {
                plan.conflicts.push(on_skipped(c, &skipped));
                skipped.insert(&r.tx_id);
                continue;
            }
        };
        let n = plan.planned.len() + 1;
        let intent = Tx {
            tx_id: format!("{merge_id}-{n}"),
            author: author.to_string(),
            branch: into.to_string(),
            base_rev: seq,
            ..rebased.tx
        };
        let link = Link::Merge {
            merge_id: merge_id.to_string(),
            from: from.to_string(),
            origin: r.tx_id.clone(),
        };
        seq += 1;
        let copy = CommittedTx {
            rev: seq,
            tx: intent.clone(),
            diff: rebased.applied.diff.clone(),
            bindings: rebased.applied.bindings.clone(),
            link: Some(Box::new(link.clone())),
        };
        copies.push((i, Step::from_committed(&copy)?));
        head = rebased.applied.project.clone();
        plan.merged.push(r.tx_id.clone());
        plan.planned.push(Planned {
            link,
            intent,
            applied: rebased.applied,
        });
    }
    if !skip_conflicting && let Some(all) = Conflicts::new(plan.conflicts.clone()) {
        return Err(all.into());
    }
    Ok(plan)
}

/// `c`, as `DEPENDS_ON_SKIPPED` when the record it blames is a skipped candidate (its inverse
/// is in the steps unmirrored).
pub(super) fn on_skipped(c: Conflict, skipped: &HashSet<&str>) -> Conflict {
    match c.against_tx.as_deref() {
        Some(t) if skipped.contains(t) => Conflict::new(
            c.tx_id,
            c.op_index,
            reasons::DEPENDS_ON_SKIPPED,
            c.against_tx,
        ),
        _ => c,
    }
}
