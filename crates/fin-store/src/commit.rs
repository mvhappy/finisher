//! `Store::commit`: dedupe by `txId`, rebase a stale tx over what was committed since its base,
//! apply it, and append it in one SQLite transaction together with the id counters it used.

use fin_core::{CommittedTx, CoreError, Tx, apply_tx, rebase_tx};
use rusqlite::TransactionBehavior;
use serde_json::{Value, json};

use crate::db::{self, Counters, SNAPSHOT_EVERY};
use crate::{Head, Store, StoreError, branch_not_found};

impl Store {
    /// Commits `tx` to its branch and returns it as committed.
    ///
    /// - A `txId` that is already committed returns that `CommittedTx` unchanged (idempotent
    ///   retries); the head does not move.
    /// - `baseRev` behind the head: the tx is rebased over the txs committed since; a conflict
    ///   is `CONFLICT` with `details.againstTx` (committed) and `details.txId` (incoming).
    /// - `baseRev` ahead of the head is `BASE_REV_AHEAD`.
    ///
    /// The stored tx is the rebased one (`baseRev` = the head it was applied to), with the diff,
    /// inverse and bindings of that same apply. Ids are allocated from the `ids` table inside the
    /// same SQLite transaction as the `txs` insert, so a failed commit consumes none.
    pub fn commit(&mut self, tx: Tx) -> Result<CommittedTx, StoreError> {
        if let Some(existing) = db::find_tx(&self.conn, &tx.tx_id)? {
            return Ok(existing);
        }
        let head = self
            .heads
            .get(&tx.branch)
            .ok_or_else(|| branch_not_found(&tx.branch))?;
        if tx.base_rev > head.rev {
            return Err(CoreError::new("BASE_REV_AHEAD", format!("baseRev {} is ahead of head {}", tx.base_rev, head.rev))
                .with_details(json!({ "txId": tx.tx_id, "baseRev": tx.base_rev, "head": head.rev, "branch": tx.branch }))
                .with_fix("read the project head and send its rev as baseRev")
                .into());
        }
        let tx = if tx.base_rev < head.rev {
            let since = db::txs_since(&self.conn, &tx.branch, tx.base_rev)?;
            rebase_tx(&tx, &since).map_err(|c| with_tx_id(CoreError::from(c), &tx.tx_id))?
        } else {
            tx
        };
        let rev = head.rev + 1;

        let txn = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ids = Counters::load(&txn)?;
        let applied = apply_tx(&head.project, &tx, &mut ids)?;
        ids.save(&txn)?;
        db::insert_tx(&txn, rev, &tx, &applied)?;
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

        self.heads.insert(
            tx.branch.clone(),
            Head {
                rev,
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

fn with_tx_id(mut e: CoreError, tx_id: &str) -> CoreError {
    if let Value::Object(map) = &mut e.details {
        map.insert("txId".to_string(), json!(tx_id));
    }
    e
}
