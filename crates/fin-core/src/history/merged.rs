//! The derived `merged` state of a branch (spec §4.1, §4.4; plan 4B Decision 21).
//!
//! Branch B is **merged** when a branch T that is not B's descendant holds a `merge` record
//! with `from = B`, and nothing of B's own (its records with edits after its fork, `merge`
//! copies it brought in included: merges are transitive) is left to merge into T: each one's
//! origin root is on T's lineage or copied there, or it is not a candidate at all (an undo
//! tied to T's records, [`candidates`]), or its origin root is dropped for T (a `drop`
//! record, [`super::dropped_for`]). Reads records' origins (`LogView::find`,
//! `merge_group`), never diffs or states. A new edit on B is a candidate again, so B is active again. A merge into a
//! descendant (updating a child from its parent) never makes the parent merged, as it never
//! drops the parent's undo stack (Decision 22).
//!
//! The store keeps the state in its branch index and recomputes it from the log alone, so a
//! follower that applies the records in any order, and a rebuild, derive the same.

use super::merge::{common_point, path_after};
use super::origin::Roots;
use super::{Lineage, LogView, candidates};
use crate::branch::BranchIndex;
use crate::error::CoreError;

/// The txIds of `branch`'s own records (seq above its fork) that a merge into `into` would
/// still copy, oldest first. Reads only lineages and records (not their diffs); empty
/// when the two share no history.
pub fn left_to_merge(
    log: &dyn LogView,
    branch: &str,
    into: &str,
) -> Result<Vec<String>, CoreError> {
    let (source_l, target_l) = (log.lineage(branch)?, log.lineage(into)?);
    let c = common_point(&source_l, &target_l);
    if c.0.is_empty() || branch == into {
        return Ok(Vec::new());
    }
    let source = path_after(log, &source_l, &c)?;
    let target = path_after(log, &target_l, &c)?;
    let roots = Roots::new(log, &target_l, &source, &target)?;
    let dropped = super::dropped_for(log, &target_l)?;
    Ok(candidates(&source, &roots, &dropped)
        .picks
        .into_iter()
        .map(|i| &source[i])
        .filter(|r| r.branch == branch)
        .map(|r| r.record.tx_id.clone())
        .collect())
}

/// The lineage of `branch` in index `ix` (as the store reads it from its `branches` table):
/// own branch first, then each ancestor through the lowest fork below it. A branch the index
/// does not hold is a root of its own; a parent loop ends the walk.
pub fn lineage_in(ix: &BranchIndex, branch: &str) -> Lineage {
    let mut segs: Vec<(String, u64, Option<u64>)> = Vec::new();
    let (mut cur, mut through) = (branch.to_string(), None::<u64>);
    loop {
        if segs.iter().any(|s| s.0 == cur) {
            return Lineage { segs };
        }
        let info = ix.get(&cur);
        let after = info.map_or(0, |i| i.fork_seq);
        segs.push((cur.clone(), after, through));
        let Some(parent) = info.and_then(|i| i.parent.clone()) else {
            return Lineage { segs };
        };
        through = Some(through.map_or(after, |t| t.min(after)));
        cur = parent;
    }
}
