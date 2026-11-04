//! `Store::commit`: dedupe by `txId`, rebase a stale tx over what was committed since its base,
//! apply it, and append it in one SQLite transaction together with the id counters it used.

use std::time::{SystemTime, UNIX_EPOCH};

use fin_core::{CommittedTx, CoreError, Tx, apply_tx, rebase_tx};
use rusqlite::TransactionBehavior;
use serde_json::{Value, json};

use crate::db::{self, Counters, SNAPSHOT_EVERY};
use crate::{Head, Store, StoreError, branch_not_found, refresh};

impl Store {
    /// Commits `tx` to its branch and returns it as committed.
    ///
    /// - An empty `txId` or empty `ops` is `INVALID_OP`, checked before anything else.
    /// - A `txId` that is already committed returns that `CommittedTx` unchanged (idempotent
    ///   retries); the head does not move.
    /// - `baseRev` behind the head: the tx is rebased over the txs committed since; a conflict
    ///   is `CONFLICT` with `details.againstTx` (committed) and `details.txId` (incoming).
    /// - `baseRev` ahead of the head is `BASE_REV_AHEAD`.
    /// - While the last import found a fork or a newer format ([`Store::read_only`]):
    ///   `PROJECT_READ_ONLY` with `details.reason`.
    /// - Without a held lease (the fence, checked right before the rows are written):
    ///   `PROJECT_READ_ONLY` with `details.reason` `LEASE_LOST`. The row records the epoch that
    ///   check returned (`txs.epoch`).
    ///
    /// The stored tx is the rebased one (`baseRev` = the head it was applied to), with the diff,
    /// inverse and bindings of that same apply. Ids are allocated from the `ids` table inside the
    /// same SQLite transaction as the `txs` insert, so a failed commit consumes none.
    pub fn commit(&mut self, tx: Tx) -> Result<CommittedTx, StoreError> {
        // Everything below runs under the write lock, against the head as stored: another
        // `Store` on this database may have committed since our cache was filled.
        check_shape(&tx)?;
        if let Some(reason) = self.import.read_only() {
            return Err(crate::import::read_only_error(&self.project_id, reason));
        }
        let txn = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = db::find_tx(&txn, &tx.tx_id)? {
            return Ok(existing);
        }
        let heads = self.heads.get_mut();
        refresh(&txn, heads, &tx.branch)?;
        let head = heads
            .get(&tx.branch)
            .ok_or_else(|| branch_not_found(&tx.branch))?;
        if tx.base_rev > head.rev {
            return Err(CoreError::new("BASE_REV_AHEAD", format!("baseRev {} is ahead of head {}", tx.base_rev, head.rev))
                .with_details(json!({ "txId": tx.tx_id, "baseRev": tx.base_rev, "head": head.rev, "branch": tx.branch }))
                .with_fix("read the project head and send its rev as baseRev")
                .into());
        }
        let tx = if tx.base_rev < head.rev {
            let since = db::txs_since(&txn, &tx.branch, tx.base_rev)?;
            rebase_tx(&tx, &since).map_err(|c| with_tx_id(CoreError::from(c), &tx.tx_id))?
        } else {
            tx
        };
        let rev = head.rev + 1;

        let mut ids = Counters::load(&txn)?;
        let applied = apply_tx(&head.project, &tx, &mut ids)?;
        ids.save(&txn)?;
        let parent = db::tip_hash(&txn, &tx.branch)?;
        // The fence, as close to the write as the epoch allows.
        let epoch = self
            .export
            .lease
            .check()
            .map_err(|e| crate::import::lease_lost_error(&self.project_id, &e))?;
        let record = db::new_record(
            rev,
            tx.clone(),
            applied.inverse.clone(),
            applied.bindings.clone(),
            parent,
            now_ms(),
        )?;
        let tip = db::insert_tx(&txn, &tx.branch, &record, &applied.diff, epoch)?;
        if rev % SNAPSHOT_EVERY == 0 {
            db::insert_snapshot(&txn, &tx.branch, rev, &applied.project)?;
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

        heads.insert(
            tx.branch.clone(),
            Head {
                rev,
                tip: Some(tip),
                project: applied.project,
            },
        );
        Ok(CommittedTx {
            rev,
            tx,
            diff: applied.diff,
            bindings: applied.bindings,
        })
    }
}

pub(crate) fn now_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(ms).unwrap_or(i64::MAX)
}

fn with_tx_id(mut e: CoreError, tx_id: &str) -> CoreError {
    if let Value::Object(map) = &mut e.details {
        map.insert("txId".to_string(), json!(tx_id));
    }
    e
}

/// A tx needs an id to dedupe on and at least one op to be worth a rev.
fn check_shape(tx: &Tx) -> Result<(), CoreError> {
    if tx.tx_id.trim().is_empty() {
        return Err(CoreError::new("INVALID_OP", "transaction txId is empty")
            .with_details(json!({ "field": "txId" }))
            .with_fix("send a non-empty txId (a ULID, or the caller's idempotency key)"));
    }
    if tx.ops.is_empty() {
        return Err(CoreError::new("INVALID_OP", "transaction has no ops")
            .with_details(json!({ "field": "ops", "txId": tx.tx_id }))
            .with_fix("send at least one op"));
    }
    Ok(())
}
