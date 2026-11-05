//! `Store::merge` (spec §4.4; plan 4B Task 13, Decisions 18–20, 26): the merge is planned by
//! `fin_core::history::plan_merge` over the write's own transaction and its `merge` records are
//! appended through the one write path, all in one SQLite transaction: all of them or none.
//! Export cuts them into segments as any records; a follower that has only a prefix of the
//! group applies that prefix like any records (each copy replays from its own intent and
//! bindings), and the rest when it arrives. Nothing is re-planned there.

use fin_core::branch::BranchState;
use fin_core::history::plan_merge;
use fin_core::{CommittedTx, CoreError, Link, Tx};
use rusqlite::params;
use serde_json::json;

use super::SqlLog;
use crate::branch::create::{check_key, key_reused};
use crate::branch::index;
use crate::commit::{check_args_hash, same_args_hash};
use crate::db;
use crate::write::{Draft, WriteCtx, writable};
use crate::{MergeReport, Store, StoreError};

/// [`Store::merge`]'s request.
///
/// - `tx_id`: the idempotency key, the `mergeId`; the records are `<mergeId>-<n>` and the
///   store dedupes on `<mergeId>-1` (Decision 26);
/// - `author`: the merging author, the records' author (Decision 20);
/// - `from`, `into`: branch references (the source may be archived; the target may not);
/// - `skip_conflicting`: merge what applies and leave the conflicting candidates;
/// - `dry_run`: plan against the local database and keep nothing, on a follower too;
/// - `args_hash`: the command input's hash, stored on the first record's intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeReq {
    pub tx_id: String,
    pub author: String,
    pub from: String,
    pub into: String,
    pub skip_conflicting: bool,
    pub dry_run: bool,
    pub args_hash: Option<String>,
}

impl Store {
    /// Merges branch `from` into `into` (spec §4.4): each candidate source record (its content
    /// records after the two lineages' common point not yet merged or picked into the target)
    /// re-run in order at the target's head, keeping its entity ids, as one `merge` record
    /// `<mergeId>-<n>`.
    ///
    /// Errors, in order: `INVALID_OP` (empty txId, bad `argsHash`); a retried mergeId answers
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
        let first = format!("{}-1", r.tx_id);
        loop {
            if let Some(report) = self.write(&first, r.dry_run, |ctx| plan(ctx, &r))? {
                return Ok(report);
            }
            // Deduped; gone again (an import rewound it since): the merge runs again.
            if let Some(existing) = db::find_tx(&self.conn, &first)? {
                return self.merged_before(&r, existing);
            }
        }
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
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT tx_id FROM record_links WHERE kind = 'merge' AND merge_id = ?1
                 ORDER BY branch, seq",
            )?;
            let rows = stmt.query_map(params![r.tx_id], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
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
            source_state: state_of(&self.conn, &from)?,
            dry_run: false,
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
    let into = writable(txn, &r.into)?.id;
    let head = ctx.head(&into)?;
    let log = SqlLog::with_head(txn, &into, head.rev, &head.project);
    let plan = plan_merge(&log, &from, &into, &r.author, &r.tx_id, r.skip_conflicting)?;
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
    Ok(MergeReport {
        merge_id: plan.merge_id,
        committed,
        merged: plan.merged,
        skipped: plan.conflicts,
        excluded: plan.excluded,
        source_state: state_of(txn, &from)?,
        dry_run: ctx.dry_run,
    })
}
