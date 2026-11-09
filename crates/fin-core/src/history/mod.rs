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
//! - [`limits`]: merges limited to what was reviewed ([`MergeLimits`], [`plan_merge_limited`]);
//!   [`review`]: the review plan's entries ([`ReviewEntry`]), from the merge planner itself.
//! - [`drops`]: what a lineage's `drop` records leave dropped ([`dropped_for`]).
//! - [`origin`]: origin chains and their roots ([`root_of`]), and what they say about a
//!   merge's records ([`Roots`]).

mod absorbs;
pub mod candidates;
pub mod checkpoint;
pub mod drops;
pub mod limits;
pub mod merge;
pub mod merged;
pub mod origin;
pub mod pick;
pub mod review;
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
pub use drops::{DropMark, Held, check_drop, dropped_for, source_roots};
pub use limits::{MAX_INCLUDE, MergeLimits};
pub use merge::{MergePlan, candidate_steps, common_point, plan_merge, plan_merge_limited};
pub use merged::{left_to_merge, lineage_in};
pub use origin::{Roots, root, root_of, target_roots};
pub use pick::{plan_cherry_pick, plan_revert};
pub use review::{EntryStatus, ReviewEntry};
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
    /// Every origin the `drop` records on `lineage` name, in lineage order
    /// ([`drops::marks_from_records`], the default, which reads the records). A store answers
    /// from its `drops` index.
    fn drops_on(&self, lineage: &Lineage) -> Result<Vec<DropMark>, CoreError> {
        drops::marks_from_records(self, lineage)
    }
    /// The measurer the planned record's re-run at the head checks caption overflow with
    /// (plan 4C Task 20); `None` (the default) measures nothing.
    fn measurer(&self) -> Option<&dyn TextMeasurer> {
        None
    }
    /// What an undo, redo or copy does with an item whose neighbours are both gone
    /// (`rebase::placement`): the store sets it per call from who asked; `Snap` by default.
    fn when_lost(&self) -> crate::rebase::placement::WhenLost {
        crate::rebase::placement::WhenLost::Snap
    }
}

/// Who wrote the records a plan names as causes: `recs` (the records the plan read: their
/// intents), else the log, else `own` (`(id, author)`: the record being planned, `id` itself
/// or a part `<id>-<n>` of it, which the log does not hold yet: an earlier copy of the same
/// merge, an earlier unit of the same `undo --to`).
pub(crate) struct Authors<'a, I> {
    pub log: &'a dyn LogView,
    pub recs: I,
    pub own: (&'a str, &'a str),
}

impl<'a, I: IntoIterator<Item = &'a Tx> + Clone> Authors<'a, I> {
    fn of(&self, tx_id: &str) -> Option<String> {
        if let Some(t) = self.recs.clone().into_iter().find(|t| t.tx_id == tx_id) {
            return Some(t.author.clone());
        }
        if let Some(r) = self.log.find(tx_id).ok().flatten() {
            return Some(r.record.intent.author);
        }
        let (id, author) = self.own;
        let part = tx_id
            .strip_prefix(id)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'));
        part.then(|| author.to_string())
    }

    /// `placements` with each cause's author.
    pub(crate) fn fill(
        &self,
        mut placements: Vec<crate::rebase::placement::Placement>,
    ) -> Vec<crate::rebase::placement::Placement> {
        for c in placements.iter_mut().flat_map(|p| p.causes.iter_mut()) {
            c.author = self.of(&c.tx_id);
        }
        placements
    }

    /// `c` with the causes of its refused placement (`details.placement`) authored.
    pub(crate) fn fill_conflict(&self, mut c: crate::rebase::Conflict) -> crate::rebase::Conflict {
        let causes = c
            .details
            .as_mut()
            .and_then(|d| d.get_mut("placement"))
            .and_then(|p| p.get_mut("causes"))
            .and_then(serde_json::Value::as_array_mut);
        for cause in causes.into_iter().flatten() {
            let tx = cause["txId"].as_str().unwrap_or_default().to_string();
            if let (Some(a), Some(m)) = (self.of(&tx), cause.as_object_mut()) {
                m.insert("author".into(), serde_json::Value::String(a));
            }
        }
        c
    }
}

/// The env a planner re-runs with at the head: the log's measurer.
/// What a copy's planner reads in the source's frame right before `rec`: the places of its
/// index ops (`rebase::places`) and the anchors of its positions (`rebase::anchors`); none
/// (and no frame read) when no op names either.
pub(crate) fn source_frame(
    log: &dyn LogView,
    rec: &LogRec,
    tx: &crate::tx::Tx,
    mode: crate::apply::ApplyMode,
) -> Result<FrameReads, CoreError> {
    let (index, at) = (
        crate::rebase::places::any(&tx.ops),
        crate::rebase::anchors::any(&tx.ops),
    );
    if !index && !at {
        return Ok(FrameReads::default());
    }
    let frame = log.state_at(&rec.branch, rec.seq.saturating_sub(1))?;
    FrameReads::read(&frame, tx, mode)
}

/// [`source_frame`] along one source branch, record after record (a merge's candidates): the
/// frame right before a record is the previous record's frame with that record replayed, so a
/// merge reads a frame from the log once per run of consecutive records, not once per
/// candidate (each `state_at` replays from the nearest snapshot).
#[derive(Default)]
pub(crate) struct SourceFrames {
    /// The last frame read: the state right before record `seq` of `branch`.
    last: Option<(String, u64, crate::model::Project)>,
}

impl SourceFrames {
    /// [`source_frame`] of `rec`, whose predecessor on its branch is `prev` (when the caller
    /// knows it).
    pub(crate) fn reads(
        &mut self,
        log: &dyn LogView,
        rec: &LogRec,
        prev: Option<&LogRec>,
        tx: &crate::tx::Tx,
        mode: crate::apply::ApplyMode,
    ) -> Result<FrameReads, CoreError> {
        let needed = crate::rebase::places::any(&tx.ops) || crate::rebase::anchors::any(&tx.ops);
        let follows = |p: &LogRec| p.branch == rec.branch && p.seq + 1 == rec.seq;
        // The frame carried from the previous record, when it is that record's own; a record
        // that does not replay (it would fail the log's own read too) falls back to the log.
        let carried = match (self.last.take(), prev) {
            (Some((b, s, f)), Some(p)) if follows(p) && b == p.branch && s == p.seq => {
                crate::replay::apply_committed(&f, &p.record.intent, &p.record.bindings)
                    .ok()
                    .map(|a| a.project)
            }
            _ => None,
        };
        // A record that reads nothing still carries the frame along (one replay), so the run
        // goes on through it.
        if !needed {
            self.last = carried.map(|f| (rec.branch.clone(), rec.seq, f));
            return Ok(FrameReads::default());
        }
        let frame = match carried {
            Some(f) => f,
            None => log.state_at(&rec.branch, rec.seq.saturating_sub(1))?,
        };
        let reads = FrameReads::read(&frame, tx, mode)?;
        self.last = Some((rec.branch.clone(), rec.seq, frame));
        Ok(reads)
    }
}

/// The places and anchors of a tx's ops, read in their own frame ([`source_frame`]).
#[derive(Debug, Default)]
pub(crate) struct FrameReads {
    pub places: Vec<Option<crate::rebase::places::Slot>>,
    pub anchors: crate::rebase::anchors::Anchors,
}

impl FrameReads {
    /// Both, read in `frame`.
    pub(crate) fn read(
        frame: &crate::model::Project,
        tx: &crate::tx::Tx,
        mode: crate::apply::ApplyMode,
    ) -> Result<FrameReads, CoreError> {
        Ok(FrameReads {
            places: crate::rebase::places::places(frame, tx, mode)?,
            anchors: crate::rebase::anchors::anchors(frame, tx, mode)?,
        })
    }
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
    /// One per item the record puts back with a ripple restore or move
    /// (`rebase::placement`), in op order; empty for other ops.
    pub placements: Vec<crate::rebase::placement::Placement>,
}
