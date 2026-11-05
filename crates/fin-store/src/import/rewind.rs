//! Rewinding an applied line that lost (`AppliedLineLost`), inside the pass's DB transaction.
//! Chain resolution stays pure: it reports the fork; this module decides whether the database
//! may take the lost records back out, and does it.
//!
//! # Why
//!
//! A holder exports with fence → `write_once`, and nothing makes that rename conditional on
//! still holding the lease: a holder paused between the two can land a segment arbitrarily
//! late, after another server took over and wrote the same seqs at a higher epoch. A follower
//! that applied the late segment before the winner's arrived would otherwise be read-only
//! (`CHAIN_FORK`) for good, while a follower that saw both at once follows the winner: the
//! two would never agree. Rewinding makes every follower end in the same state, whatever the
//! order the files arrived in.
//!
//! # When ([`eligible`]; all must hold, else the branch stays `CHAIN_FORK`)
//!
//! (a) The storage is not a sync folder. There the epoch rule never decides between two lines
//!     (the walk reports `SyncFolder` before any line can lose), so `AppliedLineLost` cannot
//!     occur; a sync folder is checked anyway.
//! (b) This server does not hold the lease now (the pass's fence check failed, or it never
//!     held it). A holder whose applied line lost would contradict its own lease.
//! (c) The winner's epoch is backed by the lease state: `lease.json`'s epoch, or a
//!     `lease.claim-*` file, is at or above it. A higher epoch nobody claimed looks like a
//!     hand copy or a bug, not a takeover.
//! (d) No other branch is based inside the lost range. (4A has only `main`; a branch whose
//!     base is not recorded counts as based inside, until 4B records bases in `meta`
//!     `branchBase.<branch>` = `{"branch", "seq"}`.)
//!
//! # What ([`run`])
//!
//! For the rows from the fork seq on (the first record after the fork's shared record):
//! 1. exported rows move to quarantine as `SUPERSEDED`, origin `local` when this server wrote
//!    them (a `segments` row in state `exported` covers the seq, or a folder segment holding
//!    the record has our `writerId`), else `folder`; unexported rows (a tail that descends
//!    from a lost record) as `DIVERGED_TAIL`, origin `local`, as for any diverged tail;
//! 2. the rows, the DB snapshots at or above the fork seq, and the `segments` rows whose last
//!    record is lost are deleted (rev 0's snapshot always remains, so the head is rebuilt
//!    from the latest snapshot below the fork plus the remaining rows);
//! 3. the id counters stay where they are (ids the lost records used are never handed out
//!    again); `idsScannedThrough` moves back so the winner's ids are scanned;
//! 4. the branch's generation is bumped and the rewind recorded ([`crate::history`]).
//!
//! The caller then rebuilds `Known` (its `known_tx` no longer holds the lost `txId`s, so a
//! winner that reuses one is no `TxIdReused`) and resolves once more in the same
//! transaction; at most one rewind per branch per pass.
//!
//! Compaction (Task 13) must never delete a segment that is not on the resolved chain: the
//! losing segment is what lets a follower that has not seen it yet decide the same way.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;

use super::apply::Ctx;
use super::quarantine::{self, Quarantined};
use crate::chain::{ForkReason, ForkSide, Resolution, SegRef};
use crate::counters::rewind_scan;
use crate::db::{self, hash_bytes, to_i64, to_u64};
use crate::history::{self, RewindInfo};
use crate::lease::{claim, record};
use crate::storage_kind::StorageKind;
use crate::{Rewind, StoreError};

/// A rewind that may go ahead.
#[derive(Debug, Clone, Copy)]
pub(super) struct Lost {
    /// The first seq that is rewound.
    from_seq: u64,
    winner_epoch: u64,
}

#[derive(Deserialize)]
struct BranchBase {
    branch: String,
    seq: u64,
}

/// Whether `res` is an applied line that lost and may be rewound (module docs).
pub(super) fn eligible(
    ctx: &Ctx,
    branch: &str,
    res: &Resolution,
    holds: bool,
    kind: StorageKind,
    project_dir: &Path,
) -> Result<Option<Lost>, StoreError> {
    let Some(fork) = res.fork.as_ref() else {
        return Ok(None);
    };
    if fork.reason != ForkReason::AppliedLineLost {
        return Ok(None);
    }
    // (a)
    debug_assert!(!kind.is_sync(), "AppliedLineLost on a sync folder");
    if kind.is_sync() {
        return Ok(None);
    }
    // (b)
    if holds {
        return Ok(None);
    }
    let txn = ctx.txn;
    let from_seq = match &fork.after {
        None => 1,
        Some(h) => {
            let seq: Option<i64> = txn
                .query_row(
                    "SELECT seq FROM txs WHERE branch = ?1 AND tx_hash = ?2",
                    params![branch, h],
                    |r| r.get(0),
                )
                .optional()?;
            match seq {
                Some(s) => to_u64(s)? + 1,
                None => return Ok(None),
            }
        }
    };
    let winner_epoch = fork
        .sides
        .iter()
        .map(|s| match s {
            ForkSide::Segment { epoch, .. } | ForkSide::LocalTail { epoch } => *epoch,
        })
        .max()
        .unwrap_or(0);
    // (d)
    for other in db::branches(txn)? {
        if other == branch {
            continue;
        }
        let base: Option<BranchBase> = db::meta(txn, &format!("branchBase.{other}"))?
            .and_then(|v| serde_json::from_str(&v).ok());
        match base {
            Some(b) if b.branch != branch || b.seq < from_seq => {}
            _ => return Ok(None),
        }
    }
    // (c), last: it reads the folder.
    if !backed(&project_dir.join(".finisher"), winner_epoch) {
        return Ok(None);
    }
    Ok(Some(Lost {
        from_seq,
        winner_epoch,
    }))
}

/// Whether `lease.json` or a claim file is at or above `epoch`. Unreadable counts as no.
fn backed(dot: &Path, epoch: u64) -> bool {
    if let Ok(record::Seen::Valid(r)) = record::read(&dot.join(record::FILE))
        && r.epoch >= epoch
    {
        return true;
    }
    claim::list(dot).is_ok_and(|claims| claims.iter().any(|c| c.epoch >= epoch))
}

struct Row {
    seq: u64,
    tx_id: String,
    tx_hash: String,
    bytes: Vec<u8>,
    epoch: u64,
    exported: bool,
    committed_at: i64,
}

/// Takes the lost rows of `branch` out (module docs, "What").
pub(super) fn run(
    ctx: &mut Ctx,
    branch: &str,
    lost: Lost,
    segs: &[SegRef],
) -> Result<Rewind, StoreError> {
    let txn = ctx.txn;
    let from = to_i64(lost.from_seq)?;
    let rows: Vec<Row> = {
        let mut stmt = txn.prepare(
            "SELECT seq, tx_id, tx_hash, record_bytes, epoch, exported, committed_at FROM txs
             WHERE branch = ?1 AND seq >= ?2 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![branch, from], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, i64>(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?;
        let mut out = vec![];
        for row in rows {
            let (seq, tx_id, tx_hash, bytes, epoch, exported, committed_at) = row?;
            out.push(Row {
                seq: to_u64(seq)?,
                tx_id,
                tx_hash,
                bytes,
                epoch: to_u64(epoch)?,
                exported,
                committed_at,
            });
        }
        out
    };
    // Seq ranges this server exported.
    let ours: Vec<(u64, u64)> = {
        let mut stmt = txn.prepare(
            "SELECT first_seq, last_seq FROM segments
             WHERE branch = ?1 AND state = 'exported' AND last_seq >= ?2",
        )?;
        let r = stmt.query_map(params![branch, from], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        r.map(|x| {
            let (a, b) = x?;
            Ok((to_u64(a)?, to_u64(b)?))
        })
        .collect::<Result<_, StoreError>>()?
    };
    let lost_hashes: HashSet<&str> = rows.iter().map(|r| r.tx_hash.as_str()).collect();
    // The folder segments holding lost records: writers, epochs, how many.
    let mut writers = BTreeSet::new();
    let mut epochs = BTreeSet::new();
    let mut ours_in_folder: HashSet<String> = HashSet::new();
    let mut plain = BTreeSet::new();
    let mut packs = BTreeSet::new();
    for s in segs {
        let held: Vec<String> = s
            .segment()
            .records
            .iter()
            .map(|b| hash_bytes(b))
            .filter(|h| lost_hashes.contains(h.as_str()))
            .collect();
        if held.is_empty() {
            continue;
        }
        let h = s.header();
        writers.insert(h.writer_id.clone());
        epochs.insert(h.epoch);
        if h.pack {
            packs.insert(s.hash().to_string());
        } else {
            plain.insert(s.hash().to_string());
        }
        if h.writer_id == ctx.server_id() {
            ours_in_folder.extend(held);
        }
    }

    let mut own = 0u32;
    for r in &rows {
        let mine = !r.exported
            || ours.iter().any(|&(a, b)| (a..=b).contains(&r.seq))
            || ours_in_folder.contains(&r.tx_hash);
        if mine {
            own += 1;
            writers.insert(ctx.server_id().to_string());
        }
        if !r.exported {
            epochs.insert(r.epoch);
        }
        let detail = if r.exported {
            format!(
                "seq {} (epoch {}) superseded by a line at epoch {}",
                r.seq, r.epoch, lost.winner_epoch
            )
        } else {
            format!("committed at epoch {} as seq {}", r.epoch, r.seq)
        };
        quarantine::upsert(
            txn,
            &Quarantined {
                tx_hash: &r.tx_hash,
                branch,
                origin: if mine { "local" } else { "folder" },
                reason: if r.exported {
                    quarantine::SUPERSEDED
                } else {
                    quarantine::DIVERGED_TAIL
                },
                detail: &detail,
                bytes: &r.bytes,
                now: ctx.now,
            },
        )?;
        ctx.removed(&r.tx_id, &r.tx_hash);
    }
    txn.execute(
        "DELETE FROM txs WHERE branch = ?1 AND seq >= ?2",
        params![branch, from],
    )?;
    txn.execute(
        "DELETE FROM snapshots WHERE branch = ?1 AND seq >= ?2",
        params![branch, from],
    )?;
    txn.execute(
        "DELETE FROM segments WHERE branch = ?1 AND last_seq >= ?2",
        params![branch, from],
    )?;
    rewind_scan(txn, branch, lost.from_seq - 1)?;

    let committed: Vec<i64> = rows
        .iter()
        .filter(|r| r.exported)
        .map(|r| r.committed_at)
        .collect();
    let span_ms = match (committed.iter().min(), committed.iter().max()) {
        (Some(a), Some(b)) => b - a,
        _ => 0,
    };
    let count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let segments = if plain.is_empty() {
        packs.len()
    } else {
        plain.len()
    };
    let info = RewindInfo {
        generation: 0,
        from_rev: lost.from_seq,
        kind: "appliedLine".into(),
        count,
        own,
        writers: writers.into_iter().collect(),
        epochs: epochs.into_iter().collect(),
        winner_epoch: lost.winner_epoch,
        segments: u32::try_from(segments).unwrap_or(u32::MAX),
        span_ms,
        at: ctx.now,
    };
    let generation = history::record(txn, branch, info)?;
    Ok(Rewind {
        branch: branch.to_string(),
        from_rev: lost.from_seq,
        quarantined: count,
        own: own > 0,
        generation,
    })
}
