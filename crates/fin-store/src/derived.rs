//! The derived history tables, recomputed from the log (plan 4B Decision 6): after any row
//! deletion (a rewind, a diverged tail, a cascade) the incremental updates no longer
//! hold, so [`recompute`] runs in the same transaction.
//!
//! The index's `merged` flags (Decision 21, `fin_core::history::merged`) are part of
//! [`compute`], and kept incrementally by [`Touched`]: every write and every import pass
//! settles the branches its records can change.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

use fin_core::branch::{Absorbed, BranchIndex, MAIN};
use fin_core::history::{DropMark, Lineage, LogRec, LogView, left_to_merge, lineage_in};
use fin_core::{CoreError, Diff, Link, Project, RecordKind};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::branch::index;
use crate::db::{self, to_i64, to_u64};

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
/// Each branch's `merged` flag follows from the `merge` records ([`merged_branches`]). Reads
/// only, and a function of the log alone, so every follower and a rebuild derive
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
    let merged = merged_branches(conn, &ix, None, false)?;
    let index = with_merged(&ix, &merged);
    Ok(Derived { index, ignored })
}

/// `ix` with exactly the branches `merged` flagged `merged`.
fn with_merged(ix: &BranchIndex, merged: &BTreeSet<String>) -> BranchIndex {
    BranchIndex::from_infos(ix.list().into_iter().map(|(_, i)| {
        let mut i = i.clone();
        i.merged = merged.contains(&i.id);
        i
    }))
}

/// Drops the `record_links` and `drops` rows whose record is gone, and writes what [`compute`] derives
/// from the log: the branch index and the inert marks (`record_links.ignored`). Runs inside
/// the caller's transaction (`import::cascade` after every rewind, diverged tail or
/// cascade).
pub(crate) fn recompute(txn: &Connection) -> Result<(), StoreError> {
    txn.execute(
        "DELETE FROM record_links WHERE tx_id NOT IN (SELECT tx_id FROM txs)",
        [],
    )?;
    crate::drops_index::forget_gone(txn)?;
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

/// The branches of `ix` that are `merged` (Decision 21), among `only` (all when `None`): a
/// branch B is when a branch T, not B's descendant, holds a `merge` record from B and
/// nothing of B's own is left to merge into T (`fin_core::history::left_to_merge`).
/// `links`: read records from their link rows ([`crate::link_records`]), not whole.
pub(crate) fn merged_branches(
    conn: &Connection,
    ix: &BranchIndex,
    only: Option<&BTreeSet<String>>,
    links: bool,
) -> Result<BTreeSet<String>, StoreError> {
    let pairs: Vec<(String, String)> = conn
        .prepare_cached(
            "SELECT DISTINCT from_branch, branch FROM record_links
             WHERE kind = 'merge' AND from_branch IS NOT NULL ORDER BY from_branch, branch",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let log = IndexLog {
        conn,
        ix,
        links,
        found: Default::default(),
        drops: Default::default(),
    };
    let mut out = BTreeSet::new();
    for (b, t) in pairs {
        if out.contains(&b)
            || only.is_some_and(|o| !o.contains(&b))
            || b == t
            || ix.get(&b).is_none()
            || ix.get(&t).is_none()
            || ix.is_descendant(&t, &b)
        {
            continue;
        }
        if left_to_merge(&log, &b, &t)?.is_empty() {
            out.insert(b);
        }
    }
    Ok(out)
}

/// The branches whose `merged` flag the records of one write or import pass can change
/// ([`Touched::record`], [`Touched::imported`]), settled by [`Touched::settle`] in the same
/// transaction. A branch's own new records never make it merged (a record added to a branch
/// never takes one of its records off the list left to merge); only copies of its records
/// elsewhere do. So, for a write, a merged branch that got records of its own is recomputed
/// (an edit a write made just clears the flag: it is left to merge), and every branch holding a
/// record of a new copy's origin root is recomputed ([`root_branches`]), with the copy's
/// source branch. A `drop` record (plan 5E) changes what is left to merge into its branch and
/// the branches forked from it later: its `from` branch and every branch holding a record of
/// a root it names are recomputed, as for a copy. An import pass applies
/// records in folder order, not in the order they were written: a copy may arrive before the
/// records it copies, or before the `branch` record of their branch. So every branch an import
/// gave records, and every branch an imported `branch` record makes, is recomputed whatever
/// its flag.
#[derive(Debug, Default)]
pub(crate) struct Touched {
    /// Branches a write gave an `edit`.
    cleared: BTreeSet<String>,
    /// Branches a write gave another record of their own (an undo tied to a merge's target is
    /// no candidate): recomputed when merged now.
    own: BTreeSet<String>,
    /// Branches an import gave records, or made: always recomputed.
    imported: BTreeSet<String>,
    /// The `from` branches of new copies and drops.
    sources: BTreeSet<String>,
    /// The `origin`s of new copies, and the roots new drops name.
    origins: BTreeSet<String>,
}

impl Touched {
    /// A record a write appended to `branch` with `link` (`None`: an edit).
    pub(crate) fn record(&mut self, branch: &str, link: Option<&Link>) {
        self.add(branch, link, true);
    }

    /// A record an import pass applied on `branch`.
    pub(crate) fn imported(&mut self, branch: &str, link: Option<&Link>) {
        self.add(branch, link, false);
    }

    fn add(&mut self, branch: &str, link: Option<&Link>, new: bool) {
        let own = if new {
            &mut self.own
        } else {
            &mut self.imported
        };
        match link {
            None | Some(Link::Edit) if new => {
                self.cleared.insert(branch.to_string());
            }
            Some(Link::Merge { from, origin, .. } | Link::CherryPick { from, origin, .. }) => {
                own.insert(branch.to_string());
                self.sources.insert(from.clone());
                self.origins.insert(origin.clone());
            }
            None
            | Some(Link::Edit | Link::Undo { .. } | Link::Redo { .. } | Link::Revert { .. }) => {
                own.insert(branch.to_string());
            }
            // What is dropped for `branch` (and the branches forked from it after the record)
            // changes: the source, and every branch holding a record of a named root.
            Some(Link::Drop {
                from,
                drops,
                undrops,
                ..
            }) => {
                self.sources.insert(from.clone());
                self.origins.extend(drops.iter().chain(undrops).cloned());
            }
            // A branch made after the copies of its records arrived (import only: a write's
            // new branch has no records yet, and no copies).
            Some(Link::Branch { branch_id, .. }) if !new => {
                self.imported.insert(branch_id.clone());
            }
            Some(_) => {}
        }
    }

    /// Writes the `merged` flag of every branch the touched records can change (type docs),
    /// as [`merged_branches`] gives it, and forgets them.
    pub(crate) fn settle(&mut self, conn: &Connection) -> Result<(), StoreError> {
        let t = std::mem::take(self);
        let merged_now = |id: &str| -> Result<bool, StoreError> {
            Ok(index::get(conn, id)?.is_some_and(|i| i.merged))
        };
        let mut recheck: BTreeSet<String> = t.sources;
        recheck.extend(t.imported);
        for id in &t.own {
            if merged_now(id)? {
                recheck.insert(id.clone());
            }
        }
        for origin in &t.origins {
            recheck.extend(root_branches(conn, origin)?);
        }
        let mut clear = BTreeSet::new();
        for id in &t.cleared {
            if !recheck.contains(id) && merged_now(id)? {
                clear.insert(id.clone());
            }
        }
        for id in &clear {
            set_merged(conn, id, false)?;
        }
        if recheck.is_empty() {
            return Ok(());
        }
        let ix = index::load(conn)?;
        let now = merged_branches(conn, &ix, Some(&recheck), true)?;
        for id in &recheck {
            if let Some(info) = ix.get(id)
                && info.merged != now.contains(id)
            {
                set_merged(conn, id, now.contains(id))?;
            }
        }
        Ok(())
    }
}

fn set_merged(conn: &Connection, id: &str, merged: bool) -> Result<(), StoreError> {
    if let Some(info) = index::get(conn, id)? {
        index::save(conn, &fin_core::branch::BranchInfo { merged, ..info })?;
    }
    Ok(())
}

/// The branches holding a record of `origin`'s root: up its chain (it, its origin, and so on,
/// while this database holds them) to the root, then every copy of the root, copies of copies
/// included. A copy of one is a copy of each, so a new copy of the root can leave nothing to
/// merge on any branch that holds one (a branch that got the root by another merge, plan 4B
/// Task 20 review).
fn root_branches(conn: &Connection, origin: &str) -> Result<BTreeSet<String>, StoreError> {
    let mut out = BTreeSet::new();
    let mut up = conn.prepare_cached(
        "SELECT t.branch, l.origin FROM txs t LEFT JOIN record_links l ON l.tx_id = t.tx_id
         WHERE t.tx_id = ?1",
    )?;
    let mut root = origin.to_string();
    for _ in 0..1_000 {
        let row: Option<(String, Option<String>)> = up
            .query_row(params![root], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((branch, next)) = row else { break };
        out.insert(branch);
        match next {
            Some(o) => root = o,
            None => break,
        }
    }
    let mut down =
        conn.prepare_cached("SELECT tx_id, branch FROM record_links WHERE origin = ?1")?;
    let mut seen = BTreeSet::from([root.clone()]);
    let mut queue = VecDeque::from([root]);
    while let Some(id) = queue.pop_front() {
        let copies: Vec<(String, String)> = down
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (tx, branch) in copies {
            out.insert(branch);
            if seen.insert(tx.clone()) {
                queue.push_back(tx);
            }
        }
    }
    Ok(out)
}

/// The log as [`left_to_merge`] reads it: lineages from an index (the one [`compute`] derives,
/// which may differ from the table), records without their diffs (never filled: verify reads
/// through it and writes nothing), and the `drop` marks along a lineage's bounds: from the
/// `drops` index with `links`, else from the `drop` records alone
/// ([`crate::drops_index::marks_parsed`]). States are never read.
struct IndexLog<'a> {
    conn: &'a Connection,
    ix: &'a BranchIndex,
    /// Records from their link rows ([`crate::link_records`]): writes and import passes,
    /// whose link rows are current. [`compute`] parses them.
    links: bool,
    /// [`LogView::find`] answers, by txId: origin roots are followed for every record of
    /// every pair, and mostly reach the same few records.
    found: std::cell::RefCell<BTreeMap<String, Option<LogRec>>>,
    /// [`LogView::drops_on`] answers, by lineage: the pairs of one target share it.
    drops: std::cell::RefCell<BTreeMap<Segs, Vec<DropMark>>>,
}

/// A lineage's segments ([`Lineage::segs`]), as a cache key.
type Segs = Vec<(String, u64, Option<u64>)>;

impl IndexLog<'_> {
    fn rec(&self, branch: String, seq: i64, bytes: &[u8]) -> Result<LogRec, StoreError> {
        let seq = to_u64(seq)?;
        Ok(LogRec {
            record: db::parse_record(Some(seq), bytes)?,
            branch,
            seq,
            diff: Diff::default(),
        })
    }
}

fn no_states() -> CoreError {
    CoreError::new("DB_ERROR", "the merged state reads no project states")
}

impl LogView for IndexLog<'_> {
    fn lineage(&self, branch: &str) -> Result<Lineage, CoreError> {
        Ok(lineage_in(self.ix, branch))
    }

    fn records(&self, branch: &str, after: u64, through: u64) -> Result<Vec<LogRec>, CoreError> {
        if self.links {
            let recs = crate::link_records::records(self.conn, branch, after, through)?;
            return Ok(recs.into_iter().map(|r| r.links_only()).collect());
        }
        let read = || -> Result<Vec<LogRec>, StoreError> {
            let mut stmt = self.conn.prepare_cached(
                "SELECT seq, record_bytes FROM txs WHERE branch = ?1 AND seq > ?2 AND seq <= ?3
                 ORDER BY seq",
            )?;
            let rows: Vec<(i64, Vec<u8>)> = stmt
                .query_map(params![branch, to_i64(after)?, to_i64(through)?], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<Result<_, _>>()?;
            rows.into_iter()
                .map(|(seq, bytes)| self.rec(branch.to_string(), seq, &bytes))
                .collect()
        };
        Ok(read()?)
    }

    fn head(&self, _: &str) -> Result<(u64, Project), CoreError> {
        Err(no_states())
    }

    fn state_at(&self, _: &str, _: u64) -> Result<Project, CoreError> {
        Err(no_states())
    }

    fn find(&self, tx_id: &str) -> Result<Option<LogRec>, CoreError> {
        let read = || -> Result<Option<LogRec>, StoreError> {
            if self.links {
                return Ok(crate::link_records::find(self.conn, tx_id)?.map(|r| r.links_only()));
            }
            let row: Option<(String, i64, Vec<u8>)> = self
                .conn
                .prepare_cached("SELECT branch, seq, record_bytes FROM txs WHERE tx_id = ?1")?
                .query_row(params![tx_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?;
            row.map(|(b, seq, bytes)| self.rec(b, seq, &bytes))
                .transpose()
        };
        if let Some(hit) = self.found.borrow().get(tx_id) {
            return Ok(hit.clone());
        }
        let rec = read()?;
        self.found
            .borrow_mut()
            .insert(tx_id.to_string(), rec.clone());
        Ok(rec)
    }

    fn merge_group(&self, merge_id: &str) -> Result<Vec<LogRec>, CoreError> {
        let ids: Vec<String> = self
            .conn
            .prepare_cached(
                "SELECT tx_id FROM record_links WHERE kind = 'merge' AND merge_id = ?1
                 ORDER BY branch, seq",
            )
            .map_err(StoreError::from)?
            .query_map(params![merge_id], |r| r.get(0))
            .map_err(StoreError::from)?
            .collect::<Result<_, _>>()
            .map_err(StoreError::from)?;
        let mut out = vec![];
        for id in ids {
            out.extend(self.find(&id)?);
        }
        Ok(out)
    }

    fn is_descendant(&self, a: &str, of: &str) -> Result<bool, CoreError> {
        Ok(self.ix.is_descendant(a, of))
    }

    fn merged_away(&self, _: &str) -> Result<HashSet<String>, CoreError> {
        Err(no_states())
    }

    fn drops_on(&self, lineage: &Lineage) -> Result<Vec<DropMark>, CoreError> {
        if let Some(hit) = self.drops.borrow().get(&lineage.segs) {
            return Ok(hit.clone());
        }
        let marks = if self.links {
            crate::drops_index::marks(self.conn, lineage)?
        } else {
            crate::drops_index::marks_parsed(self.conn, lineage)?
        };
        self.drops
            .borrow_mut()
            .insert(lineage.segs.clone(), marks.clone());
        Ok(marks)
    }
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
