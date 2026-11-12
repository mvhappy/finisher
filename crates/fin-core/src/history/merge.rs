//! Merge (spec §4.4; plan 4B Decisions 18–20, 24, 34): the source's candidates
//! ([`candidates`](super::candidates)) re-run, in order, at the target's head, each through the
//! rebase engine with full policy. Merges are transitive: `merge` copies the source brought in
//! from other branches are candidates too, unless the target already holds their origin root.
//!
//! - **Common point.** The two lineages share a branch, nearest first; the common point C is
//!   the lower of the two seqs up to which each lineage reads it ([`common_point`]). A path is
//!   a lineage's records after C, oldest first: on C's branch above C's seq, then on each
//!   branch below it in the lineage.
//! - **Steps.** A candidate's own diff is its stored diff, in its source frame (the source
//!   state right before it). Its steps lead from that frame to the target head
//!   ([`candidate_steps`]): the source path before it inverted, newest first; the target path
//!   (with its undo/redo mirrors, [`steps_from`]); the copies this merge planned so far. Each
//!   copy mirrors its inverted original, and a target record mirrors the inverted source record
//!   of the same origin root (one a copy of the other, or both copies of a third), so
//!   positions inside an original come back through its copy (Decision 24). An original with
//!   no copy (skipped, excluded) stays an unmirrored inverse: what depends on it conflicts.
//! - **Copies** keep the source's entity ids: the re-run binds the source record's bindings
//!   ([`BoundIds`]), in `Client` mode for client ops, `Internal` for inverses
//!   ([`rerun_mode`]). A `DUPLICATE_ID` there is `ALREADY_PRESENT` when both paths have the
//!   id from the same origin root, or neither binds it (a literal id older than C: a restore
//!   of the same entity), else `ID_COLLISION` ([`Binders`]; Decisions 18, 33). The copy is a
//!   `merge` record `<mergeId>-<n>` with `origin` = the source txId; its intent and record
//!   author are the merging author (Decision 20), its bindings the source's.
//! - **Conflicts.** A failing candidate is recorded and treated as skipped; planning goes on,
//!   so the full list comes back. A conflict blamed on a skipped or excluded candidate's
//!   inverse (its insert gone, say) is `DEPENDS_ON_SKIPPED`, and so is an `undo`, `redo` or
//!   `revert` whose unit holds a skipped or excluded candidate ([`unit_record_in`]): its
//!   inverse would undo what the target never got (plan 4B Task 20). Without `skip_conflicting`, any
//!   conflict makes the plan `CONFLICT` with every conflict in `details.conflicts`.
//!
//! Planning maps each candidate through O(source + target path) steps. The steps are prepared
//! once ([`Prepared`]: maps and footprints shared between candidates), so a candidate costs a
//! walk over them, not a copy of the records (plan 4B Task 20).

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::json;

use super::candidates::candidates;
use super::limits::MergeLimits;
use super::origin::Roots;
use super::review::ReviewEntry;
use super::{Lineage, LogRec, LogView, Planned};
use crate::apply::ApplyMode;
use crate::error::CoreError;
use crate::rebase::{
    Conflict, Conflicts, Mirrored, Prepared, PreparedRebase, Step, committed_footprints, reasons,
    rebase_prepared, steps_with_units,
};
use crate::record::{Link, Record, RecordKind};
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

/// The source path inverted and the target path as steps (`T`: [`Step`] or [`Prepared`]),
/// computed once per merge; each candidate takes its slice ([`Paths::steps`]).
pub(super) struct Paths<T = Step> {
    /// `inv[j]`: source record `j` inverted.
    inv: Vec<T>,
    /// The target path's steps, mirrors relative to this list.
    target: Vec<T>,
    /// `(target step, source index)`: a target record and a source record of the same origin
    /// root.
    cross: Vec<(usize, usize)>,
}

impl Paths<Step> {
    /// The same paths with their steps prepared once ([`Prepared`]).
    pub(super) fn prepared(&self) -> Paths<Prepared> {
        Paths {
            inv: self.inv.iter().map(Prepared::new).collect(),
            target: self.target.iter().map(Prepared::new).collect(),
            cross: self.cross.clone(),
        }
    }

    /// `roots`: [`Roots::new`] over `source` and `target`.
    pub(super) fn new(
        source: &[LogRec],
        target: &[LogRec],
        roots: &Roots,
    ) -> Result<Paths, CoreError> {
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
        let (target_steps, units) = steps_with_units(&committed.iter().collect::<Vec<_>>())?;
        let root_of_tx: HashMap<&str, &str> = target
            .iter()
            .zip(&roots.target)
            .map(|(t, r)| (t.record.tx_id.as_str(), r.as_str()))
            .collect();
        let mut paired = vec![false; target_steps.len()];
        for (q, s) in target_steps.iter().enumerate() {
            if let Some(m) = s.mirror_of {
                paired[m] = true;
                paired[q] = true;
            }
        }
        // A target step and a source record of the same origin root are the same edit, however
        // each got there (a copy of the other, or both copies of a third). On the target it is
        // the step that applies that root's record and is in effect: unmirrored and not an
        // inverse, so the last of a chain (merge, revert, undo of the revert: the undo's part).
        let mut live: HashMap<&str, usize> = HashMap::new();
        for (q, unit) in units.iter().enumerate() {
            if let Some(u) = unit
                && !u.inverted
                && !paired[q]
                && let Some(root) = root_of_tx.get(u.base.as_str())
            {
                live.insert(root, q);
            }
        }
        let mut cross = Vec::new();
        let mut used = HashSet::new();
        for (j, root) in roots.source.iter().enumerate() {
            if let Some(&q) = live.get(root.as_str())
                && used.insert(q)
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
}

impl<T: Mirrored> Paths<T> {
    /// The steps of source candidate `i`, with `planned` (source index, copy step) so far.
    pub(super) fn steps(&self, i: usize, planned: &[(usize, T)]) -> Vec<T> {
        let at = |j: usize| i - 1 - j;
        let mut steps: Vec<T> = Vec::with_capacity(i + self.target.len() + planned.len());
        steps.extend((0..i).rev().map(|j| self.inv[j].clone()));
        let mut paired = vec![false; i + self.target.len() + planned.len()];
        for t in &self.target {
            let mut t = t.clone();
            let m = t.mirror_of().map(|m| m + i);
            t.set_mirror_of(m);
            if let Some(m) = m {
                paired[m] = true;
                paired[steps.len()] = true;
            }
            steps.push(t);
        }
        for &(q, j) in &self.cross {
            let (a, b) = (j, i + q);
            if j < i && !paired[at(a)] && !paired[b] {
                steps[b].set_mirror_of(Some(at(a)));
                paired[at(a)] = true;
                paired[b] = true;
            }
        }
        for (j, step) in planned {
            let mut s = step.clone();
            if *j < i && !paired[at(*j)] {
                s.set_mirror_of(Some(at(*j)));
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
/// so far (source index, the copy as committed). `target_l` is the target's lineage (origin
/// roots pair same-root records). Fails as [`Step::from_committed`] or the log's reads.
pub fn candidate_steps(
    log: &dyn LogView,
    target_l: &Lineage,
    source_path: &[LogRec],
    i: usize,
    target_path: &[LogRec],
    planned: &[(usize, CommittedTx)],
) -> Result<Vec<Step>, CoreError> {
    let roots = Roots::new(log, target_l, source_path, target_path)?;
    let paths = Paths::new(source_path, target_path, &roots)?;
    let planned = planned
        .iter()
        .map(|(j, c)| Ok((*j, Step::from_committed(c)?)))
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok(paths.steps(i, &planned))
}

/// A planned merge: its id, the copies to append in order, the source txIds they copy, the
/// candidates that conflict (each skipped; empty unless `skip_conflicting`), the source
/// records left out as `DROPPED` or `UNDOES_TARGET_RECORDS` (not conflicts), and the origin
/// roots of the candidates the merge's limits leave out ([`MergeLimits`], oldest first).
#[derive(Debug, Clone, PartialEq)]
pub struct MergePlan {
    pub merge_id: String,
    pub planned: Vec<Planned>,
    pub merged: Vec<String>,
    pub conflicts: Vec<Conflict>,
    pub excluded: Vec<Conflict>,
    pub not_included: Vec<String>,
    /// One per item the copies put back with a ripple restore or move, each with its copy's
    /// txId, plus the `conflict` placements of skipped candidates (`rebase::placement`).
    pub placements: Vec<crate::rebase::placement::Placement>,
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
    let none = MergeLimits::default();
    plan_merge_limited(
        log,
        from,
        into,
        author,
        merge_id,
        skip_conflicting,
        none,
        None,
    )
}

/// [`plan_merge`] copying only what `limits` admit ([`super::limits`]); `explain` gets one
/// [`ReviewEntry`] per source record the plan decides about, before a `CONFLICT` is returned
/// ([`super::review`]).
#[allow(clippy::too_many_arguments)]
pub fn plan_merge_limited(
    log: &dyn LogView,
    from: &str,
    into: &str,
    author: &str,
    merge_id: &str,
    skip_conflicting: bool,
    limits: MergeLimits<'_>,
    explain: Option<&mut dyn FnMut(ReviewEntry)>,
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
    let roots = Roots::new(log, &target_l, &source, &target)?;
    let dropped = super::dropped_for(log, &target_l)?;
    let mut cands = candidates(&source, &roots, &dropped);
    let left = super::limits::apply(&mut cands, &source, &roots, from, &limits);
    let paths = Paths::new(&source, &target, &roots)?.prepared();
    let binders = Binders::new(&source, &roots.source, &target, &roots.target);
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
        not_included: super::limits::roots_of(&left, &roots),
        placements: Vec::new(),
    };
    let mut copies: Vec<(usize, Prepared)> = Vec::new();
    let authors = super::Authors {
        log,
        recs: target.iter().map(|r| &r.record.intent),
        own: (merge_id, author),
    };
    // Candidates not copied: excluded, or failed. What depends on them is DEPENDS_ON_SKIPPED.
    let mut skipped: HashSet<&str> = cands
        .excluded
        .iter()
        .map(|&(i, _)| source[i].record.tx_id.as_str())
        .chain(left.iter().map(|&i| source[i].record.tx_id.as_str()))
        .collect();
    let mut frames = super::SourceFrames::default();
    for &i in &cands.picks {
        let src = &source[i];
        let r = &src.record;
        // An undo, redo or revert of a unit this merge does not copy would apply an inverse
        // the target never had the original of (a clip the target deleted comes back).
        if let Some(dep) = unit_record_in(&source[..i], r, |t| skipped.contains(t)) {
            plan.conflicts.push(Conflict::new(
                &r.tx_id,
                0,
                reasons::DEPENDS_ON_SKIPPED,
                Some(dep),
            ));
            skipped.insert(&r.tx_id);
            continue;
        }
        let steps = paths.steps(i, &copies);
        let same_origin = binders.same_origin(&src.committed()?)?;
        let tx = Tx {
            tx_id: r.tx_id.clone(),
            ..r.intent.clone()
        };
        let mode = rerun_mode(r);
        let absorbs = super::absorbs::of_ops(log, src)?;
        let prev = i.checked_sub(1).map(|j| &source[j]);
        let reads = frames.reads(log, src, prev, &tx, mode)?;
        let rebase = PreparedRebase {
            tx: &tx,
            own: &src.diff,
            steps: &steps,
            same_origin: &same_origin,
            env: super::env_of(log),
            absorbs: &absorbs,
            places: &reads.places,
            anchors: &reads.anchors,
            history: Some(log.when_lost()),
        };
        let rerun = BoundIds::new(&r.intent.ops, &r.bindings)
            .map_err(|e| Conflict::from_rerun(&e, &r.tx_id, 0, None))
            .and_then(|mut ids| rebase_prepared(&head, &rebase, &mut ids, mode));
        let rebased = match rerun {
            Ok(rebased) => rebased,
            Err(c) => {
                let c = authors.fill_conflict(c);
                plan.placements.extend(conflict_placement(&c));
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
        copies.push((i, Prepared::new(&Step::from_committed(&copy)?)));
        head = rebased.applied.project.clone();
        plan.merged.push(r.tx_id.clone());
        let placements: Vec<_> = rebased
            .placements
            .into_iter()
            .map(|p| crate::rebase::placement::Placement {
                tx_id: intent.tx_id.clone(),
                ..p
            })
            .collect();
        let placements = authors.fill(placements);
        plan.placements.extend(placements.iter().cloned());
        plan.planned.push(Planned {
            link,
            intent,
            applied: rebased.applied,
            placements,
        });
    }
    if let Some(sink) = explain {
        super::review::explain(&source, &roots, &plan, &left, sink);
    }
    if !skip_conflicting && let Some(all) = Conflicts::new(plan.conflicts.clone()) {
        return Err(all.into());
    }
    Ok(plan)
}

/// The first record of the units `r` addresses (an `undo`'s units, a `redo`'s undo, a
/// `revert`'s merge group) among `before` (the source path before `r`) for which `pick`
/// holds: units named in order, each unit's records oldest first. `None` for other kinds (a
/// copy of an undo from another branch addresses nothing here).
pub(super) fn unit_record_in(
    before: &[LogRec],
    r: &Record,
    pick: impl Fn(&str) -> bool,
) -> Option<String> {
    let units: Vec<&str> = match r.kind {
        RecordKind::Undo => r.undoes.iter().flatten().map(String::as_str).collect(),
        RecordKind::Redo => r.redoes.iter().map(String::as_str).collect(),
        RecordKind::Revert => r.reverts.iter().map(String::as_str).collect(),
        _ => return None,
    };
    units.into_iter().find_map(|u| {
        before
            .iter()
            .map(|b| &b.record)
            .filter(|b| b.tx_id == u || b.merge_id.as_deref() == Some(u))
            .map(|b| b.tx_id.as_str())
            .find(|t| pick(t))
            .map(str::to_string)
    })
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

/// The mode a copy of `r` re-runs in: `Client` for an edit, and for a pick or merge copy of
/// client ops (full client policy); `Internal` for an undo, redo or revert, or a copy of one
/// (inverses may remove and restore, and name the ids they bring back).
pub(super) fn rerun_mode(r: &Record) -> ApplyMode {
    match r.kind {
        RecordKind::Edit => ApplyMode::Client,
        RecordKind::CherryPick | RecordKind::Merge
            if !r.intent.ops.iter().any(|o| o.is_internal()) =>
        {
            ApplyMode::Client
        }
        _ => ApplyMode::Internal,
    }
}

/// Which record first bound each entity id on each path, by origin root (Decisions 18, 33):
/// an id that a copy creates again, and the target holds, is `ALREADY_PRESENT` when both
/// paths have it from the same root (or neither path binds it: it is older than the common
/// point, and a restore brings back that same entity), else `ID_COLLISION`.
pub(super) struct Binders {
    source: HashMap<String, String>,
    target: HashMap<String, String>,
}

impl Binders {
    pub(super) fn new(
        source: &[LogRec],
        source_roots: &[String],
        target: &[LogRec],
        target_roots: &[String],
    ) -> Binders {
        let first = |path: &[LogRec], roots: &[String]| {
            let mut out: HashMap<String, String> = HashMap::new();
            for (r, root) in path.iter().zip(roots) {
                for id in r.record.bindings.values() {
                    out.entry(id.clone()).or_insert_with(|| root.clone());
                }
            }
            out
        };
        Binders {
            source: first(source, source_roots),
            target: first(target, target_roots),
        }
    }

    /// The ids `c` creates (bound or literal) that a duplicate makes `ALREADY_PRESENT`.
    pub(super) fn same_origin(&self, c: &CommittedTx) -> Result<BTreeSet<String>, CoreError> {
        Ok(committed_footprints(c)?
            .into_iter()
            .flat_map(|f| f.creates)
            .filter(|id| self.source.get(id) == self.target.get(id))
            .collect())
    }
}

/// The `conflict` placement a refused candidate carries (`NEIGHBOURS_GONE`'s
/// `details.placement`), for the plan's list.
fn conflict_placement(c: &Conflict) -> Option<crate::rebase::placement::Placement> {
    let p = c.details.as_ref()?.get("placement")?;
    serde_json::from_value(p.clone()).ok()
}
