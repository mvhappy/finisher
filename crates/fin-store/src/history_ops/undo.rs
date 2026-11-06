//! `Store::undo`, `Store::redo` and `Store::undo_stacks` (spec §4.5; plan 4B Tasks 12, 15): the
//! stacks are derived from the log for one author on one branch, the record is planned by
//! `fin_core::history` over the write's own transaction, and appended through the one write
//! path. A follower that imports an `undo` or `redo` record replays its intent like any
//! other record: nothing is re-planned there.

use fin_core::history::{LogView, Stacks, derive_stacks, plan_redo, plan_undo, plan_undo_to};
use fin_core::{CommittedTx, CoreError, Link, Tx};
use serde_json::json;

use super::{SqlLog, targets};
use crate::branch::create::{check_key, key_reused};
use crate::branch::index;
use crate::commit::{check_args_hash, same_args_hash};
use crate::db::{self, Counters};
use crate::write::{Draft, WriteCtx, writable};
use crate::{HistoryResult, Store, StoreError};

/// A history operation's request (`undo`, `redo`).
///
/// - `tx_id`: the idempotency key, the record's txId (Decision 26);
/// - `author`: whose stacks (Decision 25), and the record's author;
/// - `branch`: a branch reference (an id, an active name or `<name>#<id>`);
/// - `dry_run`: plan against the local database and keep nothing, on a follower too;
/// - `args_hash`: the command input's hash (`blake3:<hex>`, as for edits), stored on the
///   record's intent; a retry of the txId with another hash is `IDEM_KEY_REUSED`
///   (Decision 26);
/// - `to`: `undo --to <checkpoint>`, a checkpoint name on the branch's lineage: one undo
///   record for every unit of the author's made after it (Task 15). `None` for a plain undo;
///   a redo takes none (`INVALID_OP`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryReq {
    pub tx_id: String,
    pub author: String,
    pub branch: String,
    pub dry_run: bool,
    pub args_hash: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Undo,
    Redo,
}

impl Store {
    /// Undoes `author`'s top unit on `branch`: one `undo` record whose ops are the unit's
    /// stored inverses (or, for a unit a redo made, the redo record's), rebased over the
    /// branch's records since with full policy (spec §4.5, Decision 23). Returns the record
    /// and the unit id it undoes.
    ///
    /// Errors, in order: `INVALID_OP` (empty txId); a retried txId answers with its record
    /// (`IDEM_KEY_REUSED` when it holds another kind); `PROJECT_READ_ONLY`;
    /// `BRANCH_NOT_FOUND`, `BRANCH_ARCHIVED`; `NOTHING_TO_UNDO` (`details {branch, author}`:
    /// only the author's own records since the branch's fork count, Decision 22); `CONFLICT`
    /// (`details.conflicts`) when an op cannot be carried to the head. A dry run plans it
    /// against the local database and keeps nothing, on a follower too (Decision 26).
    ///
    /// With `to`, every unit of the author's made after that checkpoint, newest first, in one
    /// record (`fin_core::history::plan_undo_to`), all or nothing: `NOT_FOUND`
    /// (`details.kind` `checkpoint`) before `NOTHING_TO_UNDO`, and a `CONFLICT` lists every
    /// unit that cannot be carried to the head. `undone` names the units.
    pub fn undo(&mut self, r: HistoryReq) -> Result<HistoryResult, StoreError> {
        self.history(&r, Which::Undo)
    }

    /// Redoes `author`'s top undo on `branch`: one `redo` record whose ops are that undo
    /// record's stored inverse, rebased like an undo. `NOTHING_TO_REDO` when there is none
    /// (the author's newer edit clears them); otherwise as [`Store::undo`]. `undone` names
    /// the undo it reverses.
    pub fn redo(&mut self, r: HistoryReq) -> Result<HistoryResult, StoreError> {
        self.history(&r, Which::Redo)
    }

    /// `author`'s undo and redo stacks on `branch` (a reference), as `undo` and `redo` see
    /// them now. Reading may fill diffs a seeded rebuild left pending.
    pub fn undo_stacks(&self, branch: &str, author: &str) -> Result<Stacks, StoreError> {
        let id = index::resolve(&self.conn, branch)?.id;
        let log = SqlLog::new(&self.conn);
        let fork = log.lineage(&id)?.fork_seq();
        let head = db::latest_rev(&self.conn, &id)?.unwrap_or(fork);
        let own = log.records(&id, fork, head)?;
        Ok(derive_stacks(&own, author, &log.merged_away(&id)?))
    }

    fn history(&mut self, r: &HistoryReq, which: Which) -> Result<HistoryResult, StoreError> {
        check_key(&r.tx_id)?;
        check_args_hash(r.args_hash.as_deref(), &r.tx_id)?;
        if which == Which::Redo && r.to.is_some() {
            return Err(
                CoreError::new("INVALID_OP", "redo takes no checkpoint (--to)")
                    .with_details(json!({ "txId": r.tx_id, "to": r.to }))
                    .with_fix("one redo reverses one undo record, an undo --to included: fin redo")
                    .into(),
            );
        }
        loop {
            if let Some(c) = self.write(&r.tx_id, r.dry_run, |ctx| plan(ctx, r, which))? {
                return Ok(result(c, r.dry_run));
            }
            // Deduped; gone again (an import rewound it since): the operation runs again.
            if let Some(existing) = db::find_tx(&self.conn, &r.tx_id)? {
                return match (which, existing.link.as_deref()) {
                    (Which::Undo, Some(Link::Undo { .. }))
                    | (Which::Redo, Some(Link::Redo { .. })) => Ok(result(
                        same_args_hash(r.args_hash.as_deref(), existing)?,
                        false,
                    )),
                    _ => Err(key_reused(&existing)),
                };
            }
        }
    }
}

fn result(c: CommittedTx, dry_run: bool) -> HistoryResult {
    HistoryResult {
        undone: targets(c.link.as_deref()),
        committed: vec![c],
        dry_run,
        no_op: None,
    }
}

fn plan(ctx: &mut WriteCtx<'_>, r: &HistoryReq, which: Which) -> Result<CommittedTx, StoreError> {
    let txn = ctx.txn;
    super::merge_id_taken(txn, &r.tx_id)?;
    let branch = writable(txn, &r.branch)?.id;
    let mut ids = Counters::load(txn)?;
    let head = ctx.head(&branch)?;
    let log = SqlLog::with_head(txn, &branch, head.rev, &head.project);
    let planned = match (which, &r.to) {
        (Which::Undo, Some(name)) => {
            let (_, seq) = super::checkpoint::find(txn, &branch, name)?;
            plan_undo_to(&log, &branch, &r.author, seq, &r.tx_id, &mut ids)
        }
        (Which::Undo, None) => plan_undo(&log, &branch, &r.author, &r.tx_id, &mut ids),
        (Which::Redo, _) => plan_redo(&log, &branch, &r.author, &r.tx_id, &mut ids),
    }?;
    ids.save(txn)?;
    ctx.append(Draft {
        branch,
        link: planned.link,
        intent: Tx {
            args_hash: r.args_hash.clone(),
            ..planned.intent
        },
        inverse: planned.applied.inverse,
        bindings: planned.applied.bindings,
        diff: planned.applied.diff,
        project: planned.applied.project,
    })
}
