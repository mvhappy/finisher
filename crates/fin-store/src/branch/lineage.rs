//! Lineage-aware reads (plan 4B Decision 9).
//!
//! A branch's seqs continue from its fork (spec §4.1): seqs up to `forkSeq` mean the same on
//! the branch and on its parent, and the branch's own rows start at `forkSeq + 1`. So a read
//! of a branch below its fork reads the parent's history, recursively. The **lineage** of a
//! branch lists, own branch first, which branch holds which seqs: the branch's own rows after
//! its fork, then each ancestor's rows from its own fork through the lowest fork seen below
//! it.
//!
//! The lineage comes from the `branches` index (Task 8 keeps it from `branch` records). A
//! branch without an index row is a root of its own (`main` always has one).
//!
//! Two ways to the state at a past seq: replay forward from the latest snapshot at or below
//! the seq, or (for a write, which holds the head) rewind from the head through the stored
//! inverses of the records after the seq. [`state_near_head`] takes whichever touches fewer
//! records.

use std::collections::HashSet;

use fin_core::{ApplyMode, CommittedTx, CoreError, MemIds, Project, Tx, apply_tx_mode};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::db::{self, RecordRow, to_i64, to_u64};

/// The seqs one branch of a lineage holds: its rows with `after < seq <= through` (`through`
/// `None`: up to its head). `after` is the branch's fork seq, and `fork_hash` the `txHash`
/// there (`None` at 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineageSeg {
    pub branch: String,
    pub after: u64,
    pub through: Option<u64>,
    pub fork_hash: Option<String>,
}

impl LineageSeg {
    /// Whether this branch holds the row at `seq`.
    fn holds(&self, seq: u64) -> bool {
        seq > self.after && self.through.is_none_or(|t| seq <= t)
    }

    /// Whether this branch's state at `seq` is the lineage's (its own rows reach `seq`, or
    /// `seq` is its fork).
    fn covers(&self, seq: u64) -> bool {
        seq >= self.after && self.through.is_none_or(|t| seq <= t)
    }
}

/// The lineage of `branch`, own branch first (module docs). A loop in the index is
/// `DB_ERROR`.
pub(crate) fn lineage(conn: &Connection, branch: &str) -> Result<Vec<LineageSeg>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT parent, fork_seq, fork_hash FROM branches WHERE id = ?1")?;
    let mut segs: Vec<LineageSeg> = vec![];
    let mut seen = HashSet::new();
    let (mut cur, mut through) = (branch.to_string(), None::<u64>);
    loop {
        if !seen.insert(cur.clone()) {
            return Err(CoreError::new(
                "DB_ERROR",
                format!("the branch index has a parent loop through {cur:?}"),
            )
            .with_details(serde_json::json!({ "branch": branch, "loop": cur }))
            .with_fix("rebuild the project database (fin project rebuild)")
            .into());
        }
        let row: Option<(Option<String>, i64, Option<String>)> = stmt
            .query_row(params![cur], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let (parent, after, fork_hash) = match row {
            Some((p, f, h)) => (p, to_u64(f)?, h),
            None => (None, 0, None),
        };
        segs.push(LineageSeg {
            branch: cur,
            after,
            through,
            fork_hash,
        });
        let Some(parent) = parent else {
            return Ok(segs);
        };
        through = Some(through.map_or(after, |t| t.min(after)));
        cur = parent;
    }
}

/// The state of `branch` after `seq`, replayed from the latest snapshot at or below `seq` on
/// the branch of its lineage that covers `seq`. `DB_ERROR` when there is none.
pub(crate) fn state_at(conn: &Connection, branch: &str, seq: u64) -> Result<Project, StoreError> {
    state_in(conn, &lineage(conn, branch)?, seq)
}

fn state_in(conn: &Connection, segs: &[LineageSeg], seq: u64) -> Result<Project, StoreError> {
    let Some(i) = segs.iter().position(|s| s.covers(seq)) else {
        return Err(no_state(segs, seq));
    };
    let seg = &segs[i];
    let (from, mut project) = match latest_snapshot(conn, seg, seq)? {
        Some(snap) => snap,
        // No snapshot on this branch (not even at its fork): its fork state, from the parent.
        None if i + 1 < segs.len() => (seg.after, state_in(conn, &segs[i + 1..], seg.after)?),
        None => return Err(no_state(segs, seq)),
    };
    let rows = db::record_rows_in(conn, &seg.branch, from, Some(seq))?;
    check_contiguous(&seg.branch, from, Some(seq), rows.iter().map(|r| r.0))?;
    for row in rows {
        project = db::replay_row(&project, &seg.branch, &row)?.project;
    }
    Ok(project)
}

/// `seqs` (of `branch`'s rows after `after`) must be `after + 1, after + 2, …`, through
/// `through` when given: an ancestor that does not reach a child's fork, or a gap, is damage
/// (`DB_ERROR`), never a state or a history labelled with seqs it does not have.
fn check_contiguous(
    branch: &str,
    after: u64,
    through: Option<u64>,
    seqs: impl Iterator<Item = u64>,
) -> Result<(), StoreError> {
    let mut expected = after + 1;
    for seq in seqs {
        if seq != expected {
            return Err(gap(branch, expected, Some(seq)));
        }
        expected += 1;
    }
    match through {
        Some(t) if expected <= t => Err(gap(branch, expected, None)),
        _ => Ok(()),
    }
}

fn gap(branch: &str, expected: u64, found: Option<u64>) -> StoreError {
    CoreError::new(
        "DB_ERROR",
        format!("branch {branch:?} has no record at seq {expected}, which its lineage needs"),
    )
    .with_details(serde_json::json!({ "branch": branch, "seq": expected, "found": found }))
    .with_fix("rebuild the project database (fin project rebuild)")
    .into()
}

fn no_state(segs: &[LineageSeg], seq: u64) -> StoreError {
    let branch = segs.first().map_or("", |s| s.branch.as_str());
    CoreError::new(
        "DB_ERROR",
        format!("branch {branch:?} has no snapshot at or below seq {seq}"),
    )
    .into()
}

/// The latest snapshot of `seg`'s branch at or below `seq` (and not below its fork).
fn latest_snapshot(
    conn: &Connection,
    seg: &LineageSeg,
    seq: u64,
) -> Result<Option<(u64, Project)>, StoreError> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, json FROM snapshots WHERE branch = ?1 AND seq >= ?2 AND seq <= ?3
             ORDER BY seq DESC LIMIT 1",
            params![seg.branch, to_i64(seg.after)?, to_i64(seq)?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(s, json)| {
        let project = fin_core::upcast_snapshot(serde_json::from_str(&json)?)?;
        Ok((to_u64(s)?, project))
    })
    .transpose()
}

/// The state of `branch` after `seq`, given its head `head` at `head_rev`: rewound from the
/// head through the stored inverses of the `head_rev - seq` records after `seq` (on the
/// lineage) when that is no more than the records a replay from the latest snapshot at or
/// below `seq` takes ([`state_at`] otherwise, and whenever a rewind does not go through, so
/// damage is reported by the replay). Inverses are applied with structural checks only, as
/// replay applies intents: what was committed is not re-judged.
pub(crate) fn state_near_head(
    conn: &Connection,
    branch: &str,
    seq: u64,
    head_rev: u64,
    head: &Project,
) -> Result<Project, StoreError> {
    let segs = lineage(conn, branch)?;
    let snap = match segs.iter().find(|s| s.covers(seq)) {
        Some(seg) => snapshot_seq(conn, seg, seq)?,
        None => None,
    };
    let replay = snap.map(|s| seq - s);
    let rewind = head_rev.saturating_sub(seq);
    if seq <= head_rev
        && replay.is_none_or(|r| rewind <= r)
        && let Some(p) = rewind_to(conn, &segs, seq, head_rev, head)?
    {
        return Ok(p);
    }
    state_in(conn, &segs, seq)
}

fn snapshot_seq(conn: &Connection, seg: &LineageSeg, seq: u64) -> Result<Option<u64>, StoreError> {
    let s: Option<i64> = conn.query_row(
        "SELECT MAX(seq) FROM snapshots WHERE branch = ?1 AND seq >= ?2 AND seq <= ?3",
        params![seg.branch, to_i64(seg.after)?, to_i64(seq)?],
        |r| r.get(0),
    )?;
    s.map(to_u64).transpose()
}

/// `head` (at `head_rev`) with the inverses of the lineage's records after `seq` applied,
/// newest first. `None` when the rows are not exactly `seq + 1 ..= head_rev` or an inverse
/// does not apply.
pub(super) fn rewind_to(
    conn: &Connection,
    segs: &[LineageSeg],
    seq: u64,
    head_rev: u64,
    head: &Project,
) -> Result<Option<Project>, StoreError> {
    let rows = rows_since(conn, segs, seq)?;
    if !rows.iter().map(|r| r.1.0).eq(seq + 1..=head_rev) {
        return Ok(None);
    }
    let mut project = head.clone();
    for (_, (row_seq, _, bytes)) in rows.iter().rev() {
        let Ok(record) = db::parse_record(Some(*row_seq), bytes) else {
            return Ok(None);
        };
        let back = Tx {
            ops: record.inverse,
            ..record.intent
        };
        match apply_tx_mode(&project, &back, &mut MemIds::new(), ApplyMode::Replay) {
            Ok(a) => project = a.project,
            Err(_) => return Ok(None),
        }
    }
    Ok(Some(project))
}

/// Each lineage branch with the range `(lo, hi]` of seqs above `after` it holds, oldest
/// first.
fn ranges(segs: &[LineageSeg], after: u64) -> impl Iterator<Item = (&LineageSeg, u64)> {
    segs.iter()
        .rev()
        .map(move |s| (s, after.max(s.after)))
        .filter(|(s, lo)| s.through.is_none_or(|t| t > *lo))
}

/// [`records_since`] up to `through`: the records with `after < rev <= through`, in rev order
/// (`fin log`'s window, review batch L). Only the lineage branches the window reaches are read.
pub(crate) fn records_between(
    conn: &Connection,
    branch: &str,
    after: u64,
    through: u64,
) -> Result<Vec<CommittedTx>, StoreError> {
    let segs = lineage(conn, branch)?;
    let mut out = vec![];
    for (seg, lo) in ranges(&segs, after) {
        if lo >= through {
            break;
        }
        let hi = Some(seg.through.map_or(through, |t| t.min(through)));
        if crate::diffs::pending(conn, &seg.branch)?.is_some_and(|p| p > lo) {
            crate::diffs::fill(conn, &seg.branch)?;
        }
        let txs = db::txs_in(conn, &seg.branch, lo, hi)?;
        check_contiguous(&seg.branch, lo, hi, txs.iter().map(|c| c.rev))?;
        out.extend(txs);
    }
    Ok(out)
}

/// The raw rows of the lineage with `seq > after`, in order, with the branch holding each.
fn rows_since(
    conn: &Connection,
    segs: &[LineageSeg],
    after: u64,
) -> Result<Vec<(String, RecordRow)>, StoreError> {
    let mut out = vec![];
    for (seg, lo) in ranges(segs, after) {
        for row in db::record_rows_in(conn, &seg.branch, lo, seg.through)? {
            out.push((seg.branch.clone(), row));
        }
    }
    Ok(out)
}

/// Committed records on the lineage of `branch` with `rev > after`, in rev order: below the
/// fork, the ancestors' (their `tx.branch` names the ancestor). Pending diffs
/// ([`crate::diffs`]) are filled first on each branch the read reaches.
pub(crate) fn records_since(
    conn: &Connection,
    branch: &str,
    after: u64,
) -> Result<Vec<CommittedTx>, StoreError> {
    let segs = lineage(conn, branch)?;
    let mut out = vec![];
    for (seg, lo) in ranges(&segs, after) {
        if crate::diffs::pending(conn, &seg.branch)?.is_some_and(|through| through > lo) {
            crate::diffs::fill(conn, &seg.branch)?;
        }
        let txs = db::txs_in(conn, &seg.branch, lo, seg.through)?;
        check_contiguous(&seg.branch, lo, seg.through, txs.iter().map(|c| c.rev))?;
        out.extend(txs);
    }
    Ok(out)
}

/// `txHash` of the record at `seq` on the lineage of `branch`: the branch's own row, the
/// fork hash at its fork, an ancestor's row below it. `None` at 0 or when there is no row.
pub(crate) fn hash_at(
    conn: &Connection,
    branch: &str,
    seq: u64,
) -> Result<Option<String>, StoreError> {
    if seq == 0 {
        return Ok(None);
    }
    // The common case, a row of the branch's own: no index read.
    if let Some(h) = db::hash_at(conn, branch, seq)? {
        return Ok(Some(h));
    }
    for seg in lineage(conn, branch)? {
        if seg.holds(seq) {
            return db::hash_at(conn, &seg.branch, seq);
        }
        if seg.covers(seq)
            && let Some(h) = seg.fork_hash
        {
            return Ok(Some(h));
        }
    }
    Ok(None)
}

/// Hash of the newest record on the lineage of `branch`, the `parentHash` of its next record:
/// its own newest row, else its fork hash (`None` at a root's start or a fork at 0).
pub(crate) fn tip_hash(conn: &Connection, branch: &str) -> Result<Option<String>, StoreError> {
    if let Some(h) = db::tip_hash(conn, branch)? {
        return Ok(Some(h));
    }
    Ok(lineage(conn, branch)?
        .into_iter()
        .next()
        .and_then(|s| s.fork_hash))
}

#[cfg(test)]
#[path = "lineage_tests.rs"]
mod tests;
