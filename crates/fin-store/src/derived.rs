//! The derived history tables, recomputed from the log (plan 4B Decision 6): after any row
//! deletion (a rewind, a diverged tail, a cascade) the incremental updates no longer
//! hold, so [`recompute`] runs in the same transaction.

use std::collections::VecDeque;

use fin_core::RecordKind;
use fin_core::branch::{Absorbed, BranchIndex, MAIN};
use rusqlite::{Connection, params};

use crate::StoreError;
use crate::branch::index;
use crate::db::{self, to_u64};

/// Drops the `record_links` rows whose record is gone, and rebuilds the branch index from
/// the `branch` and `branchSet` records reachable from `main`, parents before children,
/// marking the records that take no effect (`record_links.ignored`): one the index refuses
/// or whose `forkHash` is not its parent's record, and a `branch` record that loses an id
/// collision (Decision 33: the higher `epoch`, then the lower `txHash`, holds the id, in any
/// fold order; a branch that holds it after another is folded again, as its rows are the
/// holder's). (`merged` is recomputed from `merge` records from Task 14 on; until then
/// nothing sets it.) Runs inside the caller's transaction (`import::cascade` after every
/// rewind, diverged tail or cascade).
pub(crate) fn recompute(txn: &Connection) -> Result<(), StoreError> {
    txn.execute(
        "DELETE FROM record_links WHERE tx_id NOT IN (SELECT tx_id FROM txs)",
        [],
    )?;
    txn.execute(
        "UPDATE record_links SET ignored = NULL WHERE ignored IS NOT NULL",
        [],
    )?;
    let kinds = [
        db::kind_text(RecordKind::Branch)?,
        db::kind_text(RecordKind::BranchSet)?,
    ];
    let mut ix = BranchIndex::with_main();
    let mut work = VecDeque::from([MAIN.to_string()]);
    let mut stmt = txn.prepare(
        "SELECT seq, record_bytes, tx_hash FROM txs WHERE branch = ?1 AND kind IN (?2, ?3)
         ORDER BY seq",
    )?;
    let mut folds = 0usize;
    while let Some(branch) = work.pop_front() {
        // A branch is folded again only when another record took its id; each record wins
        // at most once per order, so this ends. The bound only guards the loop.
        folds += 1;
        if folds > 1_000_000 {
            break;
        }
        let rows: Vec<(i64, Vec<u8>, String)> = stmt
            .query_map(params![branch, kinds[0], kinds[1]], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<Result<_, _>>()?;
        for (seq, bytes, hash) in rows {
            let seq = to_u64(seq)?;
            let rec = db::parse_record(Some(seq), &bytes)?;
            let mut next = ix.clone();
            match next.absorb(&branch, seq, &rec, &hash) {
                Err(e) => index::set_ignored(txn, &hash, Some(&index::unfit(&e.message)))?,
                Ok(Absorbed::Nothing) => {}
                Ok(Absorbed::Set(_)) => ix = next,
                Ok(Absorbed::Lost { id, winner }) => {
                    index::set_ignored(txn, &hash, Some(&index::collision(&id, &winner)))?;
                }
                Ok(Absorbed::Made(id)) => {
                    let info = next.get(&id).cloned().ok_or_else(|| index::missing(&id))?;
                    match index::bad_fork(txn, &ix, &info)? {
                        Some(why) => index::set_ignored(txn, &hash, Some(&index::unfit(&why)))?,
                        None => {
                            ix = next;
                            work.push_back(id);
                        }
                    }
                }
                Ok(Absorbed::Replaced { id, old }) => {
                    let info = next.get(&id).cloned().ok_or_else(|| index::missing(&id))?;
                    match index::bad_fork(txn, &ix, &info)? {
                        Some(why) => index::set_ignored(txn, &hash, Some(&index::unfit(&why)))?,
                        None => {
                            if let Some(r) = &old.record {
                                index::set_ignored(txn, r, Some(&index::collision(&id, &hash)))?;
                            }
                            ix = next;
                            work.push_back(id);
                        }
                    }
                }
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
