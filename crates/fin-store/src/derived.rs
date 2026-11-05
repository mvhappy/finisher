//! The derived history tables, recomputed from the log (plan 4B Decision 6): after any row
//! deletion (a rewind, a diverged tail, a cascade) the incremental updates no longer
//! hold, so [`recompute`] runs in the same transaction.

use std::collections::{BTreeMap, VecDeque};

use fin_core::RecordKind;
use fin_core::branch::{Absorbed, BranchIndex, MAIN};
use rusqlite::{Connection, params};

use crate::StoreError;
use crate::branch::index;
use crate::db::{self, to_u64};

/// What the log alone says the derived tables hold ([`compute`]).
#[derive(Debug, Clone)]
pub(crate) struct Derived {
    /// The branch index.
    pub(crate) index: BranchIndex,
    /// `txHash` → why, of every `branch` and `branchSet` record that takes no effect.
    pub(crate) ignored: BTreeMap<String, String>,
}

/// The last fold of one record ([`compute`]).
enum Outcome {
    /// The index refuses it (why).
    Unfit(String),
    /// A `branch` record that made, or took, id `0` when folded.
    Branch(String),
    /// A `branch` record that lost id `0` to the record `1` when folded.
    Lost(String, String),
    /// A `branchSet` record applied to id `0` while record `1` held it.
    Set(String, String),
}

/// Rebuilds the branch index from the `branch` and `branchSet` records reachable from
/// `main`, parents before children, and finds the records that take no effect:
/// - one the index refuses, or a `branch` record whose `forkHash` is not its parent's
///   record (`INVALID_FORMAT: …`);
/// - a `branch` record that does not hold its id in the final index: it lost an id
///   collision (Decision 33: the higher `epoch`, then the lower `txHash`, holds the id, in
///   any fold order), to whichever record holds the id at the end, also when it lost to an
///   earlier holder first (`ID_COLLISION: …`, naming the final holder);
/// - a `branchSet` record applied to a holder that lost its id later (its effect went with
///   it; named the same way).
///
/// A branch that holds an id after another is folded again, as its rows are the holder's.
/// (`merged` is recomputed from `merge` records from Task 14 on; until then nothing sets
/// it.) Reads only, and a function of the log alone, so every follower and a rebuild derive
/// the same: [`recompute`] writes the result (import after every pass that folded a `branch`
/// or `branchSet` record, and after every cascade), verify compares it.
pub(crate) fn compute(conn: &Connection) -> Result<Derived, StoreError> {
    let kinds = [
        db::kind_text(RecordKind::Branch)?,
        db::kind_text(RecordKind::BranchSet)?,
    ];
    let mut ix = BranchIndex::with_main();
    let mut last: BTreeMap<String, Outcome> = BTreeMap::new();
    let mut work = VecDeque::from([MAIN.to_string()]);
    let mut stmt = conn.prepare(
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
            let outcome = match next.absorb(&branch, seq, &rec, &hash) {
                Err(e) => Outcome::Unfit(e.message),
                Ok(Absorbed::Nothing) => continue,
                Ok(Absorbed::Set(id)) => {
                    let holder = next
                        .get(&id)
                        .and_then(|i| i.record.clone())
                        .unwrap_or_default();
                    ix = next;
                    Outcome::Set(id, holder)
                }
                Ok(Absorbed::Lost { id, winner }) => Outcome::Lost(id, winner),
                Ok(Absorbed::Made(id) | Absorbed::Replaced { id, .. }) => {
                    let info = next.get(&id).cloned().ok_or_else(|| index::missing(&id))?;
                    match index::bad_fork(conn, &ix, &info)? {
                        Some(why) => Outcome::Unfit(why),
                        None => {
                            ix = next;
                            // Folded once more only if not queued already: a fold of a
                            // queued branch would meet its children's records twice.
                            if !work.contains(&id) {
                                work.push_back(id.clone());
                            }
                            Outcome::Branch(id)
                        }
                    }
                }
            };
            last.insert(hash, outcome);
        }
    }
    let holder = |id: &str| ix.get(id).and_then(|i| i.record.clone());
    let mut ignored = BTreeMap::new();
    for (hash, outcome) in last {
        let why = match outcome {
            Outcome::Unfit(why) => Some(index::unfit(&why)),
            Outcome::Branch(id) => match holder(&id) {
                Some(h) if h != hash => Some(index::collision(&id, &h)),
                _ => None,
            },
            Outcome::Lost(id, winner) => match holder(&id) {
                Some(h) if h == hash => None,
                Some(h) => Some(index::collision(&id, &h)),
                None => Some(index::collision(&id, &winner)),
            },
            Outcome::Set(id, at) => match holder(&id) {
                Some(h) if h == at => None,
                Some(h) => Some(index::collision(&id, &h)),
                None => Some(index::collision(&id, &at)),
            },
        };
        if let Some(why) = why {
            ignored.insert(hash, why);
        }
    }
    Ok(Derived { index: ix, ignored })
}

/// Drops the `record_links` rows whose record is gone, and writes what [`compute`] derives
/// from the log: the branch index and the inert marks (`record_links.ignored`). Runs inside
/// the caller's transaction (`import::cascade` after every rewind, diverged tail or
/// cascade).
pub(crate) fn recompute(txn: &Connection) -> Result<(), StoreError> {
    txn.execute(
        "DELETE FROM record_links WHERE tx_id NOT IN (SELECT tx_id FROM txs)",
        [],
    )?;
    let d = compute(txn)?;
    write_marks(txn, &d)?;
    index::replace_all(txn, &d.index)
}

/// Writes the inert marks [`compute`] derives (`record_links.ignored`), leaving the index
/// as it is: import after a pass that folded `branch` or `branchSet` records without a
/// cascade, so a record's mark never depends on what had arrived when it was folded (a
/// record that lost to an earlier holder names the final one; a `branchSet` applied to a
/// holder that lost its id later is inert). The index such a pass keeps incrementally is
/// the one `compute` derives (debug builds check it).
pub(crate) fn remark(txn: &Connection) -> Result<(), StoreError> {
    let d = compute(txn)?;
    debug_assert_eq!(
        index::load(txn)?.list(),
        d.index.list(),
        "the incremental branch index is the one the log gives"
    );
    write_marks(txn, &d)
}

fn write_marks(txn: &Connection, d: &Derived) -> Result<(), StoreError> {
    txn.execute(
        "UPDATE record_links SET ignored = NULL WHERE ignored IS NOT NULL",
        [],
    )?;
    for (hash, why) in &d.ignored {
        index::set_ignored(txn, hash, Some(why))?;
    }
    Ok(())
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
