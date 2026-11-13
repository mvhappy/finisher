//! Merges limited to what was reviewed (sub-project 5 spec §8.4; plan 5E Decision 6): a merge
//! may name the origin roots it copies (`include`) and the newest seq of the source branch it
//! reads (`sourceUpTo`). Both are applied by [`super::plan_merge_limited`] to the candidates it
//! plans, so inside the merge's write transaction, at the heads of that moment: a commit that
//! lands on the source after a review is never copied by the review's merge.
//!
//! A candidate is copied only if its origin root is in `include` (when given) and, when it is
//! a record **of the source branch itself**, its seq is at most `sourceUpTo` (when given). A
//! candidate on an ancestor of the source is older than the source's fork and passes the seq
//! bound. The rest stay on the source, are reported as `notIncluded` (origin roots, oldest
//! first) and count as skipped: what depends on them is `DEPENDS_ON_SKIPPED`, a conflict.
//! `include: []` merges nothing. Records left out as `DROPPED` or `UNDOES_TARGET_RECORDS`
//! keep that reason, whatever the limits say.

use std::collections::{BTreeSet, HashSet};

use super::LogRec;
use super::candidates::Candidates;
use super::origin::Roots;

/// The most origin roots a merge's `include` may name: a review plan's entries at most
/// (plan 5E Decision 7), so a review merge can include every entry it lists.
pub const MAX_INCLUDE: usize = 10_000;

/// The limits of a merge (module docs); the default limits nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeLimits<'a> {
    /// The origin roots a merge may copy; `None`: any.
    pub include: Option<&'a BTreeSet<String>>,
    /// The newest seq of the source branch's own records a merge may copy; `None`: any.
    pub source_up_to: Option<u64>,
}

impl MergeLimits<'_> {
    /// Whether the candidate `rec` (origin root `root`) of a merge from `from` passes.
    fn admits(&self, rec: &LogRec, root: &str, from: &str) -> bool {
        let included = self.include.is_none_or(|i| i.contains(root));
        let in_bound = self
            .source_up_to
            .is_none_or(|n| rec.branch != from || rec.seq <= n);
        included && in_bound
    }
}

/// Takes the candidates `limits` leave out of `cands.picks` and returns their indexes into
/// `source`, in order.
pub(super) fn apply(
    cands: &mut Candidates,
    source: &[LogRec],
    roots: &Roots,
    from: &str,
    limits: &MergeLimits<'_>,
) -> Vec<usize> {
    let (picks, left): (Vec<usize>, Vec<usize>) = cands
        .picks
        .iter()
        .partition(|&&i| limits.admits(&source[i], &roots.source[i], from));
    cands.picks = picks;
    left
}

/// The origin roots of the candidates at `left`, first seen first (`MergeReport.notIncluded`).
pub(super) fn roots_of(left: &[usize], roots: &Roots) -> Vec<String> {
    let mut seen = HashSet::new();
    left.iter()
        .map(|&i| &roots.source[i])
        .filter(|r| seen.insert(r.as_str()))
        .cloned()
        .collect()
}
