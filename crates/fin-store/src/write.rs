//! The one write path (plan 4B Task 7): [`Store::write`] runs a commit or a history
//! operation in one SQLite transaction, and [`WriteCtx::append`] appends each of its records.
//!
//! A real write checks, in this order: the dedupe on its first txId, the read-only state, the
//! lease fence (the full check runs before the database lock is taken, trusted for
//! [`TRUSTS_CHECK`]), the dedupe again under the lock, the fence's answer; then runs the
//! operation, re-checks the lease in memory at the same epoch, and commits. A **dry run**
//! (Decision 26) plans against the local database in a transaction that is rolled back, with
//! no read-only, lease or fence check, so it also works on a follower. It is IMMEDIATE too:
//! planning writes (ids, filled diffs, index rows), and a DEFERRED transaction that read first
//! fails at once with `SQLITE_BUSY` when another connection (a leader beside a follower on the
//! same database) committed in between, where an IMMEDIATE one waits for the busy timeout.
//! Heads the write moved are staged in the context and adopted into the store's cache only
//! after the commit.
//!
//! Not everything a write reads stays inside it: the dedupe answer (`db::find_tx`) and plain
//! reads (`Store::ops_since`) may fill pending diffs ([`crate::diffs`]) outside any write
//! transaction, so those derived diffs persist even around a dry run on a follower. They are
//! what a replay computes, so that changes no state.

use std::collections::{BTreeMap, HashMap};

use fin_core::branch::BranchInfo;
use fin_core::{CommittedTx, CoreError, Diff, Link, Op, Project, Tx};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::branch::lineage;
use crate::db::{self, SNAPSHOT_EVERY};
use crate::{Head, Store, StoreError, branch_not_found, refresh};

/// How old a passed lease check a write may rely on. Safe because a write only touches the
/// local database: every write to the folder (each segment export) takes its own full check.
/// A record written in the last second of a lease another server has already taken becomes a
/// diverged tail, which import sets aside (spec §3.6 step 7).
pub(crate) const TRUSTS_CHECK: std::time::Duration = std::time::Duration::from_secs(1);

/// One record to append: its link, intent (`baseRev` = the head it applies to), and the
/// inverse, bindings, diff and resulting project of that apply.
#[derive(Debug, Clone)]
pub(crate) struct Draft {
    pub branch: String,
    pub link: Link,
    pub intent: Tx,
    pub inverse: Vec<Op>,
    pub bindings: BTreeMap<String, String>,
    pub diff: Diff,
    pub project: Project,
}

/// What an operation inside [`Store::write`] works with.
pub(crate) struct WriteCtx<'a> {
    pub txn: &'a Transaction<'a>,
    /// The lease epoch rows are written at; `None` in a dry run (rows at 0, rolled back).
    pub epoch: Option<u64>,
    /// Read by the history operations to shape their reports.
    pub dry_run: bool,
    /// `committedAt` of every record of this write (unix ms).
    pub now: i64,
    cache: &'a mut HashMap<String, Head>,
    staged: HashMap<String, Head>,
    /// `total_changes()` of the connection when the write began: once the transaction has
    /// changed rows (records, ids, filled diffs, index rows), what it reads is no longer
    /// committed state, so heads are not refreshed into the shared cache any more.
    changes_at_start: u64,
}

impl WriteCtx<'_> {
    /// The head of `branch` as this write sees it: moved by its own appends, else the
    /// database's (`BRANCH_NOT_FOUND` when there is none).
    pub(crate) fn head(&mut self, branch: &str) -> Result<&Head, StoreError> {
        if !self.staged.contains_key(branch) {
            if self.txn.total_changes() == self.changes_at_start {
                // Committed state only: safe to keep in the cache whatever this write does.
                refresh(self.txn, self.cache, branch)?;
                return self
                    .cache
                    .get(branch)
                    .ok_or_else(|| branch_not_found(branch).into());
            }
            // The cached head still is the branch's head in this transaction (no row of it
            // was added or replaced): use it without copying the project.
            if let Some(h) = self.cache.get(branch)
                && db::latest_rev(self.txn, branch)? == Some(h.rev)
                && lineage::hash_at(self.txn, branch, h.rev)? == h.tip
            {
                return Ok(&self.cache[branch]);
            }
            let mut one = HashMap::new();
            if let Some(h) = self.cache.get(branch) {
                one.insert(branch.to_string(), h.clone());
            }
            refresh(self.txn, &mut one, branch)?;
            let head = one.remove(branch).ok_or_else(|| branch_not_found(branch))?;
            self.staged.insert(branch.to_string(), head);
        }
        Ok(&self.staged[branch])
    }

    /// Appends `d` at the head of its branch: the record (seq = head + 1, `parentHash` = the
    /// head's tip, the fork hash at a branch's start), its `txs` row at this write's epoch
    /// and its `record_links` row, the branch index row a `branch` or `branchSet` record
    /// changes, a snapshot every [`SNAPSHOT_EVERY`] seqs, and the staged head. Returns the
    /// record as committed, with its link (`None` for an edit).
    pub(crate) fn append(&mut self, d: Draft) -> Result<CommittedTx, StoreError> {
        if d.intent.branch != d.branch {
            return Err(CoreError::new(
                "DB_ERROR",
                format!(
                    "record {} names branch {:?} but is appended to {:?}",
                    d.intent.tx_id, d.intent.branch, d.branch
                ),
            )
            .into());
        }
        let head = self.head(&d.branch)?;
        debug_assert_eq!(
            d.intent.base_rev, head.rev,
            "a draft's intent is applied at the head"
        );
        let seq = head.rev + 1;
        let parent = head.tip.clone();
        let record = db::new_record(
            seq,
            d.link,
            d.intent.clone(),
            d.inverse,
            d.bindings.clone(),
            parent,
            self.now,
        )?;
        let link = db::committed_link(&record)?;
        let tip = db::insert_tx(
            self.txn,
            &d.branch,
            &record,
            &d.diff,
            self.epoch.unwrap_or(0),
        )?;
        crate::branch::index::absorb(self.txn, &d.branch, seq, &record, &tip)?;
        if seq.is_multiple_of(SNAPSHOT_EVERY) {
            db::insert_snapshot(self.txn, &d.branch, seq, &d.project)?;
        }
        self.staged.insert(
            d.branch,
            Head {
                rev: seq,
                tip: Some(tip),
                project: d.project,
            },
        );
        Ok(CommittedTx {
            rev: seq,
            tx: d.intent,
            diff: d.diff,
            bindings: d.bindings,
            link,
        })
    }
}

/// The branch a write appends content to, for reference `r` (Decision 12): `BRANCH_NOT_FOUND`,
/// `DB_ERROR` for a branch with rows but no index row (reads take it as a root; a write
/// would grow history the log cannot explain), or `BRANCH_ARCHIVED` (`details {branch, name}`, `fix` to restore it) for an archived one
/// (Decision 13). Resolved under the write lock, before the write reads a head.
pub(crate) fn writable(conn: &Connection, r: &str) -> Result<BranchInfo, StoreError> {
    let info = crate::branch::index::resolve(conn, r)?;
    if info.parent.is_none() && info.id != crate::MAIN {
        // Rows without an index row: readable as a root, never written to (`DB_ERROR`, the
        // fix is a rebuild).
        return Err(crate::branch::index::missing(&info.id));
    }
    if info.archived {
        return Err(CoreError::new(
            "BRANCH_ARCHIVED",
            format!("branch {}#{} is archived", info.name, info.id),
        )
        .with_details(serde_json::json!({ "branch": info.id, "name": info.name }))
        .with_fix(format!("fin branch archive {} --restore", info.id))
        .into());
    }
    Ok(info)
}

/// Whether a record with `tx_id` is committed.
fn committed(conn: &Connection, tx_id: &str) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM txs WHERE tx_id = ?1")?
        .exists(params![tx_id])?)
}

impl Store {
    /// Runs `f` as one write (module docs). A record with txId `first_tx_id` already
    /// committed answers `Ok(None)` before anything else, without running `f`. Real writes:
    /// the read-only check (`PROJECT_READ_ONLY`), `lease.check_recent`, an IMMEDIATE
    /// transaction, `f`, `lease.quick_check()` at the same epoch (`PROJECT_READ_ONLY`,
    /// reason `LEASE_LOST`), commit; then the heads `f` moved are cached. `dry_run`: an
    /// IMMEDIATE transaction with no read-only, lease or fence check (`epoch: None`), `f`,
    /// rollback; nothing is cached or consumed. Any error rolls everything back.
    pub(crate) fn write<R>(
        &mut self,
        first_tx_id: &str,
        dry_run: bool,
        f: impl FnOnce(&mut WriteCtx<'_>) -> Result<R, StoreError>,
    ) -> Result<Option<R>, StoreError> {
        // A retry of a committed write is answered whatever the state (it writes nothing).
        if committed(&self.conn, first_tx_id)? {
            return Ok(None);
        }
        let fenced = if dry_run {
            None
        } else {
            if let Some(reason) = self.read_only() {
                return Err(crate::import::read_only_error(&self.project_id, reason));
            }
            // The full fence reads the folder (seconds on a slow share), so it runs before
            // the database write lock is taken; its answer is applied after the dedupe below.
            Some(self.export.lease.check_recent(TRUSTS_CHECK))
        };
        // IMMEDIATE for dry runs too (module docs).
        let txn = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if committed(&txn, first_tx_id)? {
            return Ok(None);
        }
        let epoch = fenced
            .transpose()
            .map_err(|e| crate::import::lease_lost_error(&self.project_id, &e))?;
        let mut ctx = WriteCtx {
            txn: &txn,
            epoch,
            dry_run,
            now: crate::commit::now_ms(),
            cache: self.heads.get_mut(),
            staged: HashMap::new(),
            changes_at_start: txn.total_changes(),
        };
        let out = f(&mut ctx)?;
        let WriteCtx { staged, .. } = ctx;
        let Some(epoch) = epoch else {
            txn.rollback()?;
            return Ok(Some(out));
        };
        // In memory only, under the lock: still the same lease, not lost since (the
        // heartbeat or another check may have seen a newer claim) and not past its self-fence
        // deadline.
        match self.export.lease.quick_check() {
            Ok(e) if e == epoch => {}
            Ok(e) => {
                let cause = CoreError::new(
                    "LEASE_LOST",
                    format!("the lease changed from epoch {epoch} to {e} during the write"),
                );
                return Err(crate::import::lease_lost_error(&self.project_id, &cause));
            }
            Err(e) => return Err(crate::import::lease_lost_error(&self.project_id, &e)),
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if std::mem::take(&mut self.fail_after_apply) {
            // Dropping `txn` rolls back, exactly as a crash before the commit would.
            return Err(CoreError::new(
                "TEST_FAULT",
                "injected failure after apply, before the SQLite commit",
            )
            .into());
        }
        txn.commit()?;
        self.heads.get_mut().extend(staged);
        Ok(Some(out))
    }
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
