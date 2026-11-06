//! `Store::cherry_pick` and `Store::revert` (spec §4.4; plan 4B Task 14): one record each,
//! planned by `fin_core::history::{plan_cherry_pick, plan_revert}` over the write's own
//! transaction and appended through the one write path. A request with nothing to do (the
//! target already holds the record; the merge group's effect is already gone) writes nothing
//! and answers with no records, so repeating one is harmless. A follower replays the record's
//! intent like any other: nothing is re-planned there.

use fin_core::history::{plan_cherry_pick, plan_revert};
use fin_core::{CommittedTx, Link, RecordKind, Tx};
use rusqlite::{OptionalExtension, params};

use super::SqlLog;
use crate::branch::create::{check_key, key_reused};
use crate::commit::{check_args_hash, same_args_hash};
use crate::db::{self, Counters};
use crate::write::{Draft, WriteCtx, writable};
use crate::{HistoryResult, Store, StoreError};

/// [`Store::cherry_pick`]'s request.
///
/// - `tx_id`: the idempotency key, the record's txId (Decision 26);
/// - `author`: the record's author;
/// - `origin`: the txId of the record to copy (on any branch);
/// - `into`: the branch reference to copy it onto;
/// - `dry_run`: plan against the local database and keep nothing, on a follower too;
/// - `args_hash`: the command input's hash, stored on the record's intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CherryPickReq {
    pub tx_id: String,
    pub author: String,
    pub origin: String,
    pub into: String,
    pub dry_run: bool,
    pub args_hash: Option<String>,
}

/// [`Store::revert`]'s request: as [`CherryPickReq`], with `merge_id` (the group to revert)
/// and `branch` (the reference of the branch whose history holds it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertReq {
    pub tx_id: String,
    pub author: String,
    pub merge_id: String,
    pub branch: String,
    pub dry_run: bool,
    pub args_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    CherryPick,
    Revert,
}

impl Which {
    fn kind(self) -> RecordKind {
        match self {
            Which::CherryPick => RecordKind::CherryPick,
            Which::Revert => RecordKind::Revert,
        }
    }
}

/// The parts both requests share.
struct Common<'a> {
    tx_id: &'a str,
    dry_run: bool,
    args_hash: Option<&'a str>,
}

impl Store {
    /// Copies record `origin` onto branch `into` as one `cherryPick` record (spec §4.4): the
    /// record re-run at the head through the rebase engine, keeping its entity ids, with
    /// `origin` (and `originHash`) naming its origin root. `committed` is empty when `into`
    /// already holds that root or a copy of it.
    ///
    /// Errors, in order: `INVALID_OP` (empty txId, bad `argsHash`); a retried txId answers
    /// from the log (`IDEM_KEY_REUSED` when it holds another kind, another `argsHash`);
    /// `PROJECT_READ_ONLY`; `IDEM_KEY_REUSED` when the txId is a merge group's id;
    /// `BRANCH_NOT_FOUND`, `BRANCH_ARCHIVED`; `NOT_FOUND` (`details.kind` `tx`); `INVALID_OP`
    /// (a record with no ops, no common history); `CONFLICT` (`details.conflicts`). A dry run
    /// plans against the local database and keeps nothing, on a follower too.
    pub fn cherry_pick(&mut self, r: CherryPickReq) -> Result<HistoryResult, StoreError> {
        let common = Common {
            tx_id: &r.tx_id,
            dry_run: r.dry_run,
            args_hash: r.args_hash.as_deref(),
        };
        self.pick(&common, Which::CherryPick, |ctx| {
            let txn = ctx.txn;
            let into = writable(txn, &r.into)?.id;
            let head = ctx.head(&into)?;
            let log = SqlLog::with_head(txn, &into, head.rev, &head.project);
            let Some(mut p) = plan_cherry_pick(&log, &r.origin, &into, &r.author, &r.tx_id)? else {
                return Ok(None);
            };
            if let Link::CherryPick {
                origin,
                origin_hash,
                ..
            } = &mut p.link
            {
                *origin_hash = tx_hash(txn, origin)?;
            }
            Ok(Some((into, p)))
        })
    }

    /// Reverts merge group `merge_id` on `branch` as one `revert` record (spec §4.4): the
    /// inverses of the group's records, newest first, rebased over the branch's records since
    /// the group. `undone` names the group. `committed` is empty when the group's effect is
    /// gone already (reverted or undone, and not brought back).
    ///
    /// Errors: as [`Store::cherry_pick`], with `NOT_FOUND` (`details.kind` `merge`) for a
    /// group that is unknown or not on the branch's history.
    pub fn revert(&mut self, r: RevertReq) -> Result<HistoryResult, StoreError> {
        let common = Common {
            tx_id: &r.tx_id,
            dry_run: r.dry_run,
            args_hash: r.args_hash.as_deref(),
        };
        self.pick(&common, Which::Revert, |ctx| {
            let txn = ctx.txn;
            let branch = writable(txn, &r.branch)?.id;
            let mut ids = Counters::load(txn)?;
            let head = ctx.head(&branch)?;
            let log = SqlLog::with_head(txn, &branch, head.rev, &head.project);
            let planned = plan_revert(&log, &r.merge_id, &branch, &r.author, &r.tx_id, &mut ids)?;
            ids.save(txn)?;
            Ok(planned.map(|p| (branch, p)))
        })
    }

    fn pick(
        &mut self,
        c: &Common<'_>,
        which: Which,
        plan: impl Fn(
            &mut WriteCtx<'_>,
        ) -> Result<Option<(String, fin_core::history::Planned)>, StoreError>,
    ) -> Result<HistoryResult, StoreError> {
        check_key(c.tx_id)?;
        check_args_hash(c.args_hash, c.tx_id)?;
        loop {
            let wrote = self.write(c.tx_id, c.dry_run, |ctx| {
                super::merge_id_taken(ctx.txn, c.tx_id)?;
                let Some((branch, p)) = plan(ctx)? else {
                    return Ok(None);
                };
                ctx.append(Draft {
                    branch,
                    link: p.link,
                    intent: Tx {
                        args_hash: c.args_hash.map(str::to_string),
                        ..p.intent
                    },
                    inverse: p.applied.inverse,
                    bindings: p.applied.bindings,
                    diff: p.applied.diff,
                    project: p.applied.project,
                })
                .map(Some)
            })?;
            if let Some(out) = wrote {
                return Ok(result(out, c.dry_run));
            }
            // Deduped; gone again (an import rewound it since): the operation runs again.
            if let Some(existing) = db::find_tx(&self.conn, c.tx_id)? {
                if existing.link.as_deref().map(Link::kind) != Some(which.kind()) {
                    return Err(key_reused(&existing));
                }
                return Ok(result(Some(same_args_hash(c.args_hash, existing)?), false));
            }
        }
    }
}

fn result(c: Option<CommittedTx>, dry_run: bool) -> HistoryResult {
    let undone = match c.as_ref().and_then(|c| c.link.as_deref()) {
        Some(Link::Revert { reverts }) => vec![reverts.clone()],
        _ => vec![],
    };
    HistoryResult {
        committed: c.into_iter().collect(),
        undone,
        dry_run,
    }
}

/// The `txHash` of the record `tx_id` (`None` when this database does not hold it).
fn tx_hash(conn: &rusqlite::Connection, tx_id: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT tx_hash FROM txs WHERE tx_id = ?1")?
        .query_row(params![tx_id], |r| r.get(0))
        .optional()?)
}
