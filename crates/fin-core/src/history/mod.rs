//! History planning (spec §4.4, §4.5; plan 4B Tasks 12–15): pure planners over a read-only
//! view of the log ([`LogView`]). A planner reads the records it needs, decides the record
//! to append (its [`Link`], its intent rebased to the head) and the result of applying it
//! ([`Planned`]); the store appends it through its one write path. Nothing here does I/O, so
//! the planners run in memory in the property tests and build for WASM.
//!
//! - [`undo`]: the per-author undo and redo stacks derived from a branch's own records
//!   ([`derive_stacks`]), and the `undo` / `redo` planners.
//! - [`checkpoint`]: `undo --to <checkpoint>` ([`plan_undo_to`]), one undo record for every
//!   unit after a checkpoint.
//! - [`merge`] and [`candidates`]: the merge planner ([`plan_merge`]) and which source records
//!   it copies.
//! - [`pick`]: the cherry-pick and revert planners ([`plan_cherry_pick`], [`plan_revert`]).
//! - [`merged`]: the derived `merged` state ([`left_to_merge`]).
//! - [`origin`]: origin chains and their roots ([`root_of`]), and what they say about a
//!   merge's records ([`Roots`]).

mod absorbs;
pub mod candidates;
pub mod checkpoint;
pub mod merge;
pub mod merged;
pub mod origin;
pub mod pick;
pub mod undo;

use std::collections::HashSet;

use crate::apply::ApplyEnv;
use crate::error::CoreError;
use crate::layout::TextMeasurer;
use crate::model::Project;
use crate::record::{Link, Record, RecordKind};
use crate::tx::{Applied, CommittedTx, Diff, Tx};

pub use candidates::{Candidates, candidates};
pub use checkpoint::plan_undo_to;
pub use merge::{MergePlan, candidate_steps, common_point, plan_merge};
pub use merged::{left_to_merge, lineage_in};
pub use origin::{Roots, root, root_of, target_roots};
pub use pick::{plan_cherry_pick, plan_revert};
pub use undo::{StackRec, Stacks, UnitEntry, UnitSource, derive_stacks, plan_redo, plan_undo};

/// One stored record with its place in the log and its diff (the store's `txs` row).
#[derive(Debug, Clone, PartialEq)]
pub struct LogRec {
    pub branch: String,
    pub seq: u64,
    pub record: Record,
    pub diff: Diff,
}

impl LogRec {
    /// The record as a [`CommittedTx`] (with its link, `None` for an edit), as the rebase
    /// engine's steps take it. `INVALID_FORMAT` when its kind fields do not match its kind.
    pub fn committed(&self) -> Result<CommittedTx, CoreError> {
        let link = match self.record.kind {
            RecordKind::Edit => None,
            _ => Some(Box::new(self.record.link()?)),
        };
        Ok(CommittedTx {
            rev: self.seq,
            tx: self.record.intent.clone(),
            diff: self.diff.clone(),
            bindings: self.record.bindings.clone(),
            link,
        })
    }
}

/// Which branch holds which seqs of a branch's history, own branch first: `(branch, after,
/// through)` holds the rows with `after < seq <= through` (`None`: up to its head). The own
/// entry's `after` is the branch's fork seq (0 for a root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lineage {
    pub segs: Vec<(String, u64, Option<u64>)>,
}

impl Lineage {
    /// The fork seq of the lineage's own branch: its own records are the seqs above it.
    pub fn fork_seq(&self) -> u64 {
        self.segs.first().map_or(0, |s| s.1)
    }
}

/// A read-only view of a project's log, as the planners read it. Every method answers from
/// the same state (the store implements it over its write transaction).
pub trait LogView {
    /// The lineage of `branch` ([`Lineage`]). `BRANCH_NOT_FOUND` for an unknown branch.
    fn lineage(&self, branch: &str) -> Result<Lineage, CoreError>;
    /// `branch`'s own records (not its ancestors') with `after < seq <= through`, oldest
    /// first.
    fn records(&self, branch: &str, after: u64, through: u64) -> Result<Vec<LogRec>, CoreError>;
    /// [`LogView::records`] as the undo stacks read them ([`StackRec`]: kinds, authors and
    /// links only). A store answers from its link table without parsing records.
    fn stack_records(
        &self,
        branch: &str,
        after: u64,
        through: u64,
    ) -> Result<Vec<StackRec>, CoreError> {
        Ok(self
            .records(branch, after, through)?
            .iter()
            .map(StackRec::of)
            .collect())
    }
    /// The head seq of `branch` and the project there.
    fn head(&self, branch: &str) -> Result<(u64, Project), CoreError>;
    /// The project on `branch` after `seq` (below its fork: its ancestors' state).
    fn state_at(&self, branch: &str, seq: u64) -> Result<Project, CoreError>;
    /// The record with `tx_id`, on whatever branch holds it.
    fn find(&self, tx_id: &str) -> Result<Option<LogRec>, CoreError>;
    /// The `merge` records of the group `merge_id`, in seq order.
    fn merge_group(&self, merge_id: &str) -> Result<Vec<LogRec>, CoreError>;
    /// Whether branch `a` descends from `of` (a child, grandchild, …; never itself).
    fn is_descendant(&self, a: &str, of: &str) -> Result<bool, CoreError>;
    /// txIds of `branch`'s own records that are the origin of a `merge` record with
    /// `from = branch` on a branch that is not a descendant of `branch` (Decision 22).
    /// Cherry-picks never count.
    fn merged_away(&self, branch: &str) -> Result<HashSet<String>, CoreError>;
    /// The measurer the planned record's re-run at the head checks caption overflow with
    /// (plan 4C Task 20); `None` (the default) measures nothing.
    fn measurer(&self) -> Option<&dyn TextMeasurer> {
        None
    }
}

/// The env a planner re-runs with at the head: the log's measurer.
/// The places of a copy's index ops (`rebase::places`), read in the source's frame right
/// before `rec`; none (and no frame read) when no op names an index.
pub(crate) fn source_places(
    log: &dyn LogView,
    rec: &LogRec,
    tx: &crate::tx::Tx,
    mode: crate::apply::ApplyMode,
) -> Result<Vec<Option<crate::rebase::places::Place>>, CoreError> {
    if !crate::rebase::places::any(&tx.ops) {
        return Ok(Vec::new());
    }
    let frame = log.state_at(&rec.branch, rec.seq.saturating_sub(1))?;
    crate::rebase::places::places(&frame, tx, mode)
}

pub(crate) fn env_of(log: &dyn LogView) -> ApplyEnv<'_> {
    ApplyEnv::measuring(log.measurer())
}

/// What the cherry-pick and revert planners decide: a record to append, or nothing to do and
/// why (`ALREADY_PRESENT`: the target holds the record's origin root; `ALREADY_REVERTED`: the
/// merge group's effect is gone there, [`crate::rebase::reasons`]).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // one per request, moved out at once (as `Planned` is)
pub enum Outcome {
    Append(Planned),
    Nothing(&'static str),
}

impl Outcome {
    /// The record to append, if any.
    pub fn planned(self) -> Option<Planned> {
        match self {
            Outcome::Append(p) => Some(p),
            Outcome::Nothing(_) => None,
        }
    }
}

/// A planned record: its kind and links, its intent (at the head: `baseRev` = the head seq,
/// positions rebased) and the result of applying that intent at the head.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub link: Link,
    pub intent: Tx,
    pub applied: Applied,
}
