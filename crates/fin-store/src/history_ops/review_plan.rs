//! `Store::review_plan` (plan 5E Decision 7): the change list of a review, a merge of `from`
//! into `into` planned at the current heads and explained entry by entry
//! (`fin_core::history::plan_merge_limited`'s explain sink), so a review and its merge can
//! never disagree on a status. A read: it plans inside a transaction that is rolled back
//! (IMMEDIATE, as a dry run's, `crate::write`), with no read-only, lease or guard check, so it
//! works on a follower; the heads it reports are read in the same transaction.

use fin_core::history::{MergeLimits, ReviewEntry, plan_merge_limited};
use rusqlite::{Transaction, TransactionBehavior};

use super::SqlLog;
use crate::branch::index;
use crate::wire::{ReviewHead, ReviewPlan};
use crate::{Store, StoreError, branch_not_found, refresh};

/// The most entries (and conflicts) a review plan holds; the rest are cut (`truncated`).
pub const REVIEW_PLAN_MAX: usize = 10_000;

impl Store {
    /// What a merge of `from` into `into` (branch references) would do at their heads now
    /// ([`ReviewPlan`]), at most [`REVIEW_PLAN_MAX`] entries.
    ///
    /// Errors: `BRANCH_NOT_FOUND`; `INVALID_OP` (a branch into itself, no common history);
    /// what the log's reads fail with. Archived branches are planned like any.
    pub fn review_plan(&self, from: &str, into: &str) -> Result<ReviewPlan, StoreError> {
        self.review_plan_capped(from, into, REVIEW_PLAN_MAX)
    }

    /// [`Store::review_plan`] holding at most `cap` entries and conflicts (tests).
    #[doc(hidden)]
    pub fn review_plan_capped(
        &self,
        from: &str,
        into: &str,
        cap: usize,
    ) -> Result<ReviewPlan, StoreError> {
        // Rolled back on drop: planning may fill derived rows, which a replay computes again.
        let txn = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let from = index::resolve(&txn, from)?.id;
        let into = index::resolve(&txn, into)?.id;
        let (source, target) = {
            let mut heads = self.heads.borrow_mut();
            refresh(&txn, &mut heads, &from)?;
            refresh(&txn, &mut heads, &into)?;
            let at = |b: &str| heads.get(b).cloned().ok_or_else(|| branch_not_found(b));
            (at(&from)?, at(&into)?)
        };
        let measurer = self.measurer.clone();
        let log = SqlLog::with_head(&txn, &into, target.rev, &target.project)
            .measuring(measurer.as_deref());
        let (mut entries, mut truncated) = (Vec::new(), false);
        let mut sink = |e: ReviewEntry| {
            if entries.len() < cap {
                entries.push(e);
            } else {
                truncated = true;
            }
        };
        let none = MergeLimits::default();
        let plan = plan_merge_limited(
            &log,
            &from,
            &into,
            "",
            "review",
            true,
            none,
            Some(&mut sink),
        )?;
        let mut conflicts = plan.conflicts;
        if conflicts.len() > cap {
            conflicts.truncate(cap);
            truncated = true;
        }
        let head = |branch: String, h: &crate::Head| ReviewHead {
            branch,
            seq: h.rev,
            hash: h.tip.clone(),
        };
        Ok(ReviewPlan {
            source: head(from, &source),
            target: head(into, &target),
            entries,
            conflicts,
            truncated,
        })
    }
}
