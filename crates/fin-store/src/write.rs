//! The one write path (plan 4B Task 7): [`Store::write`] runs a commit or a history
//! operation in one SQLite transaction, and [`WriteCtx::append`] appends each of its records.
//!
//! A real write checks, in this order: the dedupe on its first txId, the read-only state, the
//! lease fence (the full check runs before the database lock is taken, trusted for
//! [`TRUSTS_CHECK`]), the dedupe again under the lock, the fence's answer; then runs the
//! operation, re-checks the lease in memory at the same epoch, and commits. A **dry run**
//! (Decision 26) plans against the local database in a transaction that is rolled back, with
//! no read-only, lease or fence check, so it also works on a follower. Heads the write moved
//! are staged in the context and adopted into the store's cache only after the commit.

use std::collections::{BTreeMap, HashMap};

use fin_core::{CommittedTx, CoreError, Diff, Link, Op, Project, Tx};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

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
    /// Read by the history operations (Task 8 on) to shape their reports.
    #[cfg_attr(not(test), allow(dead_code))]
    pub dry_run: bool,
    /// `committedAt` of every record of this write (unix ms).
    pub now: i64,
    cache: &'a mut HashMap<String, Head>,
    staged: HashMap<String, Head>,
    /// Whether this write has written rows: the cache then no longer matches what the
    /// transaction reads, so heads are built on the side.
    written: bool,
}

impl WriteCtx<'_> {
    /// The head of `branch` as this write sees it: moved by its own appends, else the
    /// database's (`BRANCH_NOT_FOUND` when there is none).
    pub(crate) fn head(&mut self, branch: &str) -> Result<&Head, StoreError> {
        if !self.staged.contains_key(branch) {
            if !self.written {
                // Committed state only: safe to keep in the cache whatever this write does.
                refresh(self.txn, self.cache, branch)?;
                return self
                    .cache
                    .get(branch)
                    .ok_or_else(|| branch_not_found(branch).into());
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
    /// and its `record_links` row, a snapshot every [`SNAPSHOT_EVERY`] seqs, and the staged
    /// head. Returns the record as committed, with its link (`None` for an edit).
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
        if seq.is_multiple_of(SNAPSHOT_EVERY) {
            db::insert_snapshot(self.txn, &d.branch, seq, &d.project)?;
        }
        self.written = true;
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
    /// reason `LEASE_LOST`), commit; then the heads `f` moved are cached. `dry_run`: a
    /// DEFERRED transaction with no read-only, lease or fence check (`epoch: None`), `f`,
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
        let behavior = if dry_run {
            TransactionBehavior::Deferred
        } else {
            TransactionBehavior::Immediate
        };
        let txn = self.conn.transaction_with_behavior(behavior)?;
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
            written: false,
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
