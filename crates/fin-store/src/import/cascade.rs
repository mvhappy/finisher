//! Rewinds and diverged tails cascade to descendant branches (plan 4B Decision 7), inside the
//! pass's DB transaction.
//!
//! # When
//!
//! The rows a rewind ([`super::rewind`]) or a diverged tail ([`super::apply`] step 2) takes out
//! of a branch include the `branch` record of a child (equivalently: the child's fork point is
//! not on the winning line, since a `branch` record is appended above its fork). A child whose
//! `branch` record survives forks below the lost rows and is untouched.
//!
//! # What ([`cascade`])
//!
//! The child, and recursively every descendant, is rewound whole, parents first:
//! - every row goes to quarantine by the 4A origin rule (exported: `SUPERSEDED`, unexported:
//!   `DIVERGED_TAIL`; origin `local` when this server wrote it, [`super::rewind::take_rows`]),
//!   or under the [`Cause`]'s own reason (a `branch` record voided by another rule: the
//!   entry point is [`subtree`] then [`cascade`], whatever decided the loss);
//! - its snapshots (the fork snapshot included) and `segments` rows are deleted, and so are
//!   its `meta` `idsScannedThrough` and `diffsPending` (a later branch under the same id is
//!   another branch); the id counters stay where they are;
//! - its generation is bumped and the rewind recorded from rev 0: the whole branch is gone,
//!   below its fork too (the parent's lost rows may lie there, and a branch the winner made
//!   under the same id may fork elsewhere), so a caller holding its old generation is refused
//!   whatever its base (`HISTORY_REWOUND`), and a `Rewound` event ends its subscribers;
//! - then (also when no child was lost) [`crate::derived::recompute`] drops the
//!   `record_links` rows whose record is gone and rebuilds the branch index, so a lost
//!   `branchSet` is forgotten and the child's index row goes.
//!
//! Why: followers must converge whatever the arrival order. A follower that never applied the
//! lost line sets the child's folder directory aside with its `branch` record
//! ([`super::aside::leftovers`], Decision 8); one that applied it cascades here, and its
//! directory then becomes the same leftover (the rows `upsert` keeps `SUPERSEDED` or
//! `DIVERGED_TAIL`). Refusing would bring back a permanent `CHAIN_FORK`.

use std::collections::{HashMap, VecDeque};

use rusqlite::{Connection, OptionalExtension, params};

use super::ctx::Ctx;
use super::rewind::take_rows;
use crate::chain::SegRef;
use crate::db::to_i64;
use crate::history;
use crate::names::hash12;
use crate::{Rewind, StoreError};

/// A descendant rewound with its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Child {
    pub(super) id: String,
    /// The `txHash` of the `branch` record that made it.
    record: String,
}

/// Why the children go. The cascade does not care how the loss was decided: a rewind of an
/// applied line, a diverged tail, or (for later callers) a `branch` record voided by another
/// rule ([`Cause::voided`]).
pub(super) struct Cause {
    /// The recorded rewind kind (`history::RewindInfo::kind`).
    kind: &'static str,
    /// The winner's epoch (0 for a diverged tail).
    winner_epoch: u64,
    /// The quarantine reason of every row; `None`: the 4A rule (exported `SUPERSEDED`,
    /// unexported `DIVERGED_TAIL`).
    reason: Option<&'static str>,
}

impl Cause {
    pub(super) fn applied_line(winner_epoch: u64) -> Cause {
        Cause {
            kind: "appliedLine",
            winner_epoch,
            reason: None,
        }
    }

    pub(super) fn diverged_tail() -> Cause {
        Cause {
            kind: "divergedTail",
            winner_epoch: 0,
            reason: None,
        }
    }

    /// The `branch` record stays, but no longer makes its branch (e.g. it lost an id
    /// collision): every row of the subtree goes to quarantine as `reason`.
    pub(super) fn voided(kind: &'static str, winner_epoch: u64, reason: &'static str) -> Cause {
        Cause {
            kind,
            winner_epoch,
            reason: Some(reason),
        }
    }
}

/// The `branch` records' `txHash` when the parent still holds it (`id` itself when not).
const CHILD: &str = "SELECT b.id, COALESCE(t.tx_hash, b.id) FROM branches b
     LEFT JOIN txs t ON t.branch = b.parent AND t.seq = b.created_seq";

/// The descendants of `branch` whose `branch` record is at or above `from_seq` there, and
/// recursively all of theirs, parents first (from the index, which follows the log). Read
/// before the rows are taken out.
pub(super) fn lost_children(
    conn: &Connection,
    branch: &str,
    from_seq: u64,
) -> Result<Vec<Child>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "{CHILD} WHERE b.parent = ?1 AND b.created_seq >= ?2 ORDER BY b.created_seq"
    ))?;
    let first: Vec<Child> = stmt
        .query_map(params![branch, to_i64(from_seq)?], |r| {
            Ok(Child {
                id: r.get(0)?,
                record: r.get(1)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    descend(conn, first)
}

/// Branch `id` and all its descendants, parents first: the subtree a voided `branch` record
/// takes with it (pass it to [`cascade`]). Empty when the index does not know `id`.
pub(super) fn subtree(conn: &Connection, id: &str) -> Result<Vec<Child>, StoreError> {
    let root: Option<Child> = conn
        .query_row(&format!("{CHILD} WHERE b.id = ?1"), params![id], |r| {
            Ok(Child {
                id: r.get(0)?,
                record: r.get(1)?,
            })
        })
        .optional()?;
    descend(conn, root.into_iter().collect())
}

/// `roots` and, recursively, every child of theirs, parents first.
fn descend(conn: &Connection, roots: Vec<Child>) -> Result<Vec<Child>, StoreError> {
    let mut kids = conn.prepare(&format!(
        "{CHILD} WHERE b.parent = ?1 ORDER BY b.created_seq"
    ))?;
    let mut out = vec![];
    let mut work: VecDeque<Child> = roots.into();
    while let Some(c) = work.pop_front() {
        let rows = kids.query_map(params![c.id], |r| {
            Ok(Child {
                id: r.get(0)?,
                record: r.get(1)?,
            })
        })?;
        for row in rows {
            work.push_back(row?);
        }
        out.push(c);
    }
    Ok(out)
}

/// Rewinds every branch in `lost` whole (module docs), parents first as [`lost_children`] and
/// [`subtree`] list them, then recomputes the derived tables (always, also for an empty
/// `lost`: the caller took rows out); returns one [`Rewind`] per branch. `dirs` are the
/// folder segments by branch.
pub(super) fn cascade(
    ctx: &mut Ctx,
    dirs: &HashMap<String, Vec<SegRef>>,
    lost: &[Child],
    cause: &Cause,
) -> Result<Vec<Rewind>, StoreError> {
    let mut out = vec![];
    for child in lost {
        let segs = dirs.get(&child.id).map_or(&[][..], Vec::as_slice);
        let short = hash12(&child.record).unwrap_or(&child.record).to_string();
        let superseded = |seq: u64, epoch: u64| {
            format!("seq {seq} (epoch {epoch}): its branch record {short} was set aside")
        };
        let taken = take_rows(ctx, &child.id, 0, segs, cause.reason, &superseded)?;
        let txn = ctx.txn;
        txn.execute(
            "DELETE FROM meta WHERE key IN (?1, ?2)",
            params![
                crate::counters::scanned_key(&child.id),
                crate::diffs::pending_key(&child.id)
            ],
        )?;
        let (count, own) = (taken.count, taken.own);
        let info = taken.info(0, cause.kind, cause.winner_epoch, ctx.now);
        let generation = history::record(txn, &child.id, info)?;
        out.push(Rewind {
            branch: child.id.clone(),
            from_rev: 0,
            quarantined: count,
            own: own > 0,
            generation,
        });
    }
    crate::derived::recompute(ctx.txn)?;
    Ok(out)
}
