//! `Store::merge` (spec §4.4; plan 4B Task 13, Decisions 18–20, 26): the merge is planned by
//! `fin_core::history::plan_merge_limited` over the write's own transaction (so `include` and
//! `sourceUpTo`, plan 5E Decision 6, bound the candidates of that moment) and its `merge` records are
//! appended through the one write path, all in one SQLite transaction: all of them or none.
//! Export cuts them into segments as any records; a follower that has only a prefix of the
//! group applies that prefix like any records (each copy replays from its own intent and
//! bindings), and the rest when it arrives. Nothing is re-planned there.

use std::collections::BTreeSet;

use fin_core::branch::BranchState;
use fin_core::history::{MAX_INCLUDE, MergeLimits, plan_merge_limited};
use fin_core::{CommittedTx, CoreError, Link, Tx};
use serde_json::json;

use super::{SqlLog, origins};
use crate::branch::create::{check_key, key_reused};
use crate::branch::index;
use crate::commit::{check_args_hash, same_args_hash};
use crate::db;
use crate::write::{Draft, WriteCtx};
use crate::{MergeReport, Store, StoreError};

/// [`Store::merge`]'s request.
///
/// - `tx_id`: the idempotency key, the `mergeId`; the records are `<mergeId>-<n>` and the
///   store dedupes on `<mergeId>-1` (Decision 26);
/// - `author`: the merging author, the records' author (Decision 20);
/// - `from`, `into`: branch references (the source may be archived; the target may not);
/// - `skip_conflicting`: merge what applies and leave the conflicting candidates;
/// - `include`: the origin roots the merge may copy (at most `MAX_INCLUDE`; `[]` copies
///   nothing); `source_up_to`: the newest seq of the source branch's own records it may copy
///   (plan 5E Decision 6). What they leave out is `notIncluded`;
/// - `dry_run`: plan against the local database and keep nothing, on a follower too;
/// - `args_hash`: the command input's hash, stored on the first record's intent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MergeReq {
    pub tx_id: String,
    pub author: String,
    pub from: String,
    pub into: String,
    #[serde(default)]
    pub skip_conflicting: bool,
    #[serde(default)]
    pub include: Option<Vec<String>>,
    #[serde(default)]
    pub source_up_to: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
    pub args_hash: Option<String>,
}

impl Store {
    /// Merges branch `from` into `into` (spec §4.4): each candidate source record (its records
    /// with edits after the two lineages' common point, `merge` copies from other branches
    /// included, whose origin root the target does not hold yet; Decision 34) re-run in order
    /// at the target's head, keeping its entity ids, as one `merge` record `<mergeId>-<n>`.
    /// Nothing to merge writes nothing, so a later request with the same mergeId merges what
    /// arrived since.
    ///
    /// Errors, in order: `INVALID_OP` (empty txId, bad `argsHash`, more than `MAX_INCLUDE`
    /// ids in `include`); a retried mergeId answers
    /// from the log (`IDEM_KEY_REUSED` when `<mergeId>-1` holds another kind of record or
    /// another `argsHash`); `PROJECT_READ_ONLY`; `IDEM_KEY_REUSED` when the mergeId itself or
    /// a `<mergeId>-<n>` is another record's txId; `BRANCH_NOT_FOUND`, `BRANCH_ARCHIVED` (the
    /// target); `INVALID_OP` (a branch into itself, no common history); `CONFLICT` with every
    /// conflicting candidate in `details.conflicts`, unless `skip_conflicting`. Nothing is
    /// written on any error. A dry run plans against the local database and keeps nothing, on
    /// a follower too (Decision 26).
    pub fn merge(&mut self, r: MergeReq) -> Result<MergeReport, StoreError> {
        check_key(&r.tx_id)?;
        check_args_hash(r.args_hash.as_deref(), &r.tx_id)?;
        check_include(r.include.as_deref())?;
        let first = format!("{}-1", r.tx_id);
        loop {
            if let Some(report) = self.write(&first, r.dry_run, |ctx| plan(ctx, &r))? {
                return self.merge_origins(report);
            }
            // Deduped; gone again (an import rewound it since): the merge runs again.
            if let Some(existing) = db::find_tx(&self.conn, &first)? {
                let report = self.merged_before(&r, existing)?;
                return self.merge_origins(report);
            }
        }
    }

    /// `report` with `origins`: what its copies stand for (7A Decision 27).
    fn merge_origins(&self, mut report: MergeReport) -> Result<MergeReport, StoreError> {
        report.origins = origins::chains(&self.conn, report.merged.iter().cloned())?;
        Ok(report)
    }

    /// The answer to a retried merge: its group as committed.
    fn merged_before(
        &self,
        r: &MergeReq,
        existing: CommittedTx,
    ) -> Result<MergeReport, StoreError> {
        let from = match existing.link.as_deref() {
            Some(Link::Merge { merge_id, from, .. }) if *merge_id == r.tx_id => from.clone(),
            _ => return Err(key_reused(&existing)),
        };
        same_args_hash(r.args_hash.as_deref(), existing)?;
        let ids = origins::merge_group(&self.conn, &r.tx_id)?;
        let mut committed = Vec::with_capacity(ids.len());
        for id in ids {
            committed.extend(db::find_tx(&self.conn, &id)?);
        }
        Ok(MergeReport {
            merge_id: r.tx_id.clone(),
            merged: committed.iter().filter_map(origin).collect(),
            committed,
            skipped: Vec::new(),
            excluded: Vec::new(),
            not_included: Vec::new(),
            source_state: state_of(&self.conn, &from)?,
            dry_run: false,
            origins: Vec::new(),
        })
    }
}

fn origin(c: &CommittedTx) -> Option<String> {
    match c.link.as_deref() {
        Some(Link::Merge { origin, .. }) => Some(origin.clone()),
        _ => None,
    }
}

fn state_of(conn: &rusqlite::Connection, id: &str) -> Result<BranchState, StoreError> {
    Ok(index::get(conn, id)?.map_or(BranchState::Active, |i| i.state()))
}

/// `INVALID_OP` for an `include` of more than [`MAX_INCLUDE`] ids.
fn check_include(include: Option<&[String]>) -> Result<(), StoreError> {
    let n = include.map_or(0, <[String]>::len);
    if n <= MAX_INCLUDE {
        return Ok(());
    }
    Err(CoreError::new(
        "INVALID_OP",
        format!("include names {n} origins; a merge takes at most {MAX_INCLUDE}"),
    )
    .with_details(json!({ "field": "include", "count": n, "max": MAX_INCLUDE }))
    .with_fix("merge in several steps, or review the branch (fin review)")
    .into())
}

/// A txId the merge would write that another record holds already: `IDEM_KEY_REUSED`.
fn taken(conn: &rusqlite::Connection, tx_id: &str, merge_id: &str) -> Result<(), StoreError> {
    let Some(existing) = db::find_tx(conn, tx_id)? else {
        return Ok(());
    };
    Err(CoreError::new(
        "IDEM_KEY_REUSED",
        format!("merge {merge_id} would write txId {tx_id}, which another record holds"),
    )
    .with_details(json!({
        "txId": tx_id,
        "mergeId": merge_id,
        "rev": existing.rev,
        "branch": existing.tx.branch,
    }))
    .with_fix("use a new idempotency key (mergeId) for the merge")
    .into())
}

fn plan(ctx: &mut WriteCtx<'_>, r: &MergeReq) -> Result<MergeReport, StoreError> {
    let txn = ctx.txn;
    // The mergeId names the group; it must not also be a record's txId.
    taken(txn, &r.tx_id, &r.tx_id)?;
    let from = index::resolve(txn, &r.from)?.id;
    let into = ctx.target(&r.into)?.id;
    let measurer = ctx.measurer.clone();
    let head = ctx.head(&into)?;
    let log = SqlLog::with_head(txn, &into, head.rev, &head.project).measuring(measurer.as_deref());
    let include: Option<BTreeSet<String>> = r.include.as_ref().map(|i| i.iter().cloned().collect());
    let limits = MergeLimits {
        include: include.as_ref(),
        source_up_to: r.source_up_to,
    };
    let (author, id) = (&r.author, &r.tx_id);
    let plan = plan_merge_limited(
        &log,
        &from,
        &into,
        author,
        id,
        r.skip_conflicting,
        limits,
        None,
    )?;
    let mut committed = Vec::with_capacity(plan.planned.len());
    for (n, p) in plan.planned.into_iter().enumerate() {
        taken(txn, &p.intent.tx_id, &r.tx_id)?;
        let intent = Tx {
            args_hash: if n == 0 { r.args_hash.clone() } else { None },
            ..p.intent
        };
        committed.push(ctx.append(Draft {
            branch: into.clone(),
            link: p.link,
            intent,
            inverse: p.applied.inverse,
            bindings: p.applied.bindings,
            diff: p.applied.diff,
            project: p.applied.project,
        })?);
    }
    // The source's state after the copies (Decision 21).
    ctx.settle()?;
    Ok(MergeReport {
        merge_id: plan.merge_id,
        committed,
        merged: plan.merged,
        skipped: plan.conflicts,
        excluded: plan.excluded,
        not_included: plan.not_included,
        source_state: state_of(txn, &from)?,
        dry_run: ctx.dry_run,
        origins: Vec::new(),
    })
}
