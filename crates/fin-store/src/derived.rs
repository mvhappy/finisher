//! The derived history tables, recomputed from the log (plan 4B Decision 6): after any row
//! deletion (a rewind, a diverged tail, a cascade; Task 10) the incremental updates no longer
//! hold, so [`recompute`] runs in the same transaction.

use std::collections::VecDeque;

use fin_core::RecordKind;
use fin_core::branch::{BranchIndex, MAIN};
use rusqlite::{Transaction, params};

use crate::StoreError;
use crate::branch::index;
use crate::db::{self, to_u64};

/// Drops the `record_links` rows whose record is gone, and rebuilds the branch index from
/// the `branch` and `branchSet` records reachable from `main`, parents before children.
/// (`merged` is recomputed from `merge` records from Task 14 on; until then nothing sets it.)
#[cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))]
pub(crate) fn recompute(txn: &Transaction) -> Result<(), StoreError> {
    txn.execute(
        "DELETE FROM record_links WHERE tx_id NOT IN (SELECT tx_id FROM txs)",
        [],
    )?;
    let kinds = [
        db::kind_text(RecordKind::Branch)?,
        db::kind_text(RecordKind::BranchSet)?,
    ];
    let mut ix = BranchIndex::with_main();
    let mut work = VecDeque::from([MAIN.to_string()]);
    let mut stmt = txn.prepare(
        "SELECT seq, record_bytes FROM txs WHERE branch = ?1 AND kind IN (?2, ?3) ORDER BY seq",
    )?;
    while let Some(branch) = work.pop_front() {
        let rows = stmt.query_map(params![branch, kinds[0], kinds[1]], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (seq, bytes) = row?;
            let seq = to_u64(seq)?;
            let rec = db::parse_record(Some(seq), &bytes)?;
            let changed = ix.absorb(&branch, seq, &rec)?;
            if let (RecordKind::Branch, Some(child)) = (rec.kind, changed) {
                work.push_back(child);
            }
        }
    }
    index::replace_all(txn, &ix)
}

#[cfg(any(test, feature = "test-hooks"))]
impl crate::Store {
    /// Test hook: runs [`recompute`] in a transaction of its own.
    #[doc(hidden)]
    pub fn recompute_derived_for_test(&mut self) -> Result<(), StoreError> {
        let txn = self.conn.transaction()?;
        recompute(&txn)?;
        txn.commit()?;
        Ok(())
    }
}
