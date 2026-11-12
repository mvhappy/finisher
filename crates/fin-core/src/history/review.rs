//! The review plan's entries (sub-project 5 spec §8.1; plan 5E Decision 7): what a merge of a
//! source into a target would do with each source record, as [`super::plan_merge_limited`]
//! decides it. The planner hands its explain sink one [`ReviewEntry`] per candidate and one
//! per source record the target already holds, oldest first (along the source path), after
//! planning, from the plan itself: the change list and the merge can never disagree on a
//! status.

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::LogRec;
use super::merge::MergePlan;
use super::origin::Roots;
use crate::rebase::{Conflict, reasons};
use crate::record::RecordKind;

/// What a merge does with a source record (the review's `status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EntryStatus {
    /// Copied by the merge.
    WillMerge,
    /// The target holds its origin root already (merged, picked, or its own).
    AlreadyPresent,
    /// Its origin root is dropped for the target (`DROPPED`).
    Dropped,
    /// Left out, not a conflict: `UNDOES_TARGET_RECORDS`, or `NOT_INCLUDED` by the merge's
    /// limits (`reason` says which).
    Excluded,
    /// A conflict: it depends on a record the merge does not copy (`DEPENDS_ON_SKIPPED`).
    DependsOnSkipped,
    /// A conflict of another reason (`reason`, `blame`).
    Conflict,
}

/// One entry of a review plan: a source record (`tx_id` on `branch` at `seq`) and its origin
/// root (`origin`, what decisions and `include` name), its kind, record author, number of
/// intent ops and commit time (unix ms), and what the merge does with it. `reason`: the
/// conflict or exclusion reason (absent for `willMerge` and `alreadyPresent`); `blame`: the
/// record a conflict is blamed on, when there is one. `actor`: the agent actor the server
/// knows for the record (sub-project 7A's tracker), never set by the planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewEntry {
    pub origin: String,
    pub tx_id: String,
    pub branch: String,
    pub seq: u64,
    pub kind: RecordKind,
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Value>,
    pub ops_count: usize,
    pub committed_at: i64,
    pub status: EntryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blame: Option<String>,
}

/// Hands `sink` the entries of `plan` (planned over `source` with `roots`; `left`: the indexes
/// its limits left out) in source order (module docs).
pub(super) fn explain(
    source: &[LogRec],
    roots: &Roots,
    plan: &MergePlan,
    left: &[usize],
    sink: &mut dyn FnMut(ReviewEntry),
) {
    let merged: HashSet<&str> = plan.merged.iter().map(String::as_str).collect();
    let mut failed: HashMap<&str, &Conflict> = HashMap::new();
    for c in plan.conflicts.iter().chain(&plan.excluded) {
        failed.entry(c.tx_id.as_str()).or_insert(c);
    }
    let left: HashSet<usize> = left.iter().copied().collect();
    for (i, rec) in source.iter().enumerate() {
        let r = &rec.record;
        if !(r.kind.is_content() || r.kind == RecordKind::Merge) {
            continue;
        }
        let (status, reason, blame) = if roots.present[i] {
            (EntryStatus::AlreadyPresent, None, None)
        } else if left.contains(&i) {
            let reason = Some(reasons::NOT_INCLUDED.to_string());
            (EntryStatus::Excluded, reason, None)
        } else if merged.contains(r.tx_id.as_str()) {
            (EntryStatus::WillMerge, None, None)
        } else if let Some(c) = failed.get(r.tx_id.as_str()) {
            let status = match c.reason.as_str() {
                reasons::DROPPED => EntryStatus::Dropped,
                reasons::UNDOES_TARGET_RECORDS => EntryStatus::Excluded,
                reasons::DEPENDS_ON_SKIPPED => EntryStatus::DependsOnSkipped,
                _ => EntryStatus::Conflict,
            };
            (status, Some(c.reason.clone()), c.against_tx.clone())
        } else {
            continue;
        };
        sink(ReviewEntry {
            origin: roots.source[i].clone(),
            tx_id: r.tx_id.clone(),
            branch: rec.branch.clone(),
            seq: rec.seq,
            kind: r.kind,
            author: r.author.clone(),
            actor: None,
            ops_count: r.intent.ops.len(),
            committed_at: r.committed_at,
            status,
            reason,
            blame,
        });
    }
}
