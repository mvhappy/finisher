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
//!     held it), or it holds it at epoch `E` and every lost row is below `E` (`txs.epoch`).
//!     A holder at `E` meeting a winner above its lost line has itself lost if anything at
//!     `E` is in the lost range (refuse: the folder decides who holds, not this pass);
//!     otherwise the lost range holds only older epochs, as when a follower that applied a
//!     late line takes the lease before the winner's files reach it. Without this a
//!     holder would stay `CHAIN_FORK` while heartbeating and freeze the project. Its own
//!     older unexported rows are set aside as a diverged tail.
//! (c) The winner's epoch is backed by the lease state: `lease.json`'s epoch, or a
//!     `lease.claim-*` file, is at or above it. A higher epoch nobody claimed looks like a
//!     hand copy or a bug, not a takeover. The caller reads both before the pass takes the
//!     database lock ([`lease_top`]) and passes the highest.
//! (d) No branch without an index row (a root made by hand) is based inside the lost range:
//!     it is based where `meta` `branchBase.<branch>` = `{"branch", "seq"}` says, else
//!     inside. A branch with an index row is never "based inside" in that sense: a child
//!     whose `branch` record is among the lost rows is rewound whole with it (plan 4B
//!     Decision 7, [`super::cascade`]); one whose `branch` record survives forks below the
//!     lost range and is untouched. For (b) the cascaded children's rows count as lost rows.
//!
//! # What ([`run`])
//!
//! For the rows from the fork seq on (the first record after the fork's shared record):
//! 1. exported rows move to quarantine as `SUPERSEDED`, origin `local` when this server wrote
//!    them (a `segments` row in state `exported` covers the seq, or a folder segment holding
//!    the record has our `writerId` or one this database had before a `SERVER_ID_RENEWED`,
//!    [`history::server_ids`]; a compaction pack never counts: it carries other writers'
//!    records, and its rows are `imported`), else `folder`; unexported rows (a tail that
//!    descends from a lost record) as `DIVERGED_TAIL`, origin `local`, as for any diverged
//!    tail;
//! 2. the rows, the DB snapshots at or above the fork seq, and the `segments` rows whose last
//!    record is lost are deleted (rev 0's snapshot always remains, so the head is rebuilt
//!    from the latest snapshot below the fork plus the remaining rows). That includes a
//!    pack that spans the fork: it holds lost records, so no export may continue from it.
//!    The pack file stays in the folder (compaction never deletes a pack, and deletes only
//!    segments on the resolved chain), and the winner's first segment still resolves
//!    through the pack's `replaces` (it holds the shared record);
//! 3. the id counters stay where they are (ids the lost records used are never handed out
//!    again); `idsScannedThrough` moves back so the winner's ids are scanned;
//! 4. the branch's generation is bumped and the rewind recorded ([`crate::history`]);
//! 5. the children whose `branch` record was lost are rewound whole, recursively
//!    ([`super::cascade`]), and the derived tables are recomputed (`crate::derived`).
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

use super::cascade::{self, Child};
use super::ctx::Ctx;
use super::quarantine::{self, Quarantined};
use crate::chain::{ForkReason, ForkSide, Resolution, SegRef};
use crate::counters::rewind_scan;
use crate::db::{self, hash_bytes, to_i64, to_u64};
use crate::history::{self, RewindInfo};
use crate::lease::{claim, record};
use crate::storage_kind::StorageKind;

/// The highest epoch `lease.json` or any claim file names in `dot` (`None`: neither could be
/// read). Folder I/O: called before the pass's transaction.
pub(super) fn lease_top(dot: &Path) -> Option<u64> {
    let rec = match record::read(&dot.join(record::FILE)) {
        Ok(record::Seen::Valid(r)) => Some(r.epoch),
        _ => None,
    };
    let claims = claim::list(dot)
        .ok()
        .and_then(|c| c.iter().map(|c| c.epoch).max());
    rec.max(claims)
}
use crate::{Rewind, StoreError};

/// A rewind that may go ahead.
#[derive(Debug, Clone)]
pub(super) struct Lost {
    /// The first seq that is rewound.
    from_seq: u64,
    winner_epoch: u64,
    /// The descendants rewound with it ([`cascade::lost_children`]), parents first.
    children: Vec<Child>,
}

impl Lost {
    pub(super) fn winner_epoch(&self) -> u64 {
        self.winner_epoch
    }

    pub(super) fn children(&self) -> &[Child] {
        &self.children
    }
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
    held: Option<u64>,
    lease_top: Option<u64>,
    kind: StorageKind,
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
    let txn = ctx.txn;
    let from_seq = match &fork.after {
        // The branch start: seq 1 on `main`, the first seq after the fork on a 4B branch
        // (its fork snapshot is not part of the lost range).
        None => super::known::fork_point(txn, branch)?.map_or(0, |f| f.seq) + 1,
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
    let children = cascade::lost_children(txn, branch, from_seq)?;
    // (b), the cascaded children's rows included.
    if let Some(e) = held {
        let e = to_i64(e)?;
        let mut at_ours: i64 = txn.query_row(
            "SELECT COUNT(*) FROM txs WHERE branch = ?1 AND seq >= ?2 AND epoch >= ?3",
            params![branch, to_i64(from_seq)?, e],
            |r| r.get(0),
        )?;
        for c in &children {
            at_ours += txn.query_row(
                "SELECT COUNT(*) FROM txs WHERE branch = ?1 AND epoch >= ?2",
                params![c.id, e],
                |r| r.get::<_, i64>(0),
            )?;
        }
        if at_ours > 0 {
            return Ok(None);
        }
    }
    // (d) A branch with an index row is based where its `branch` record says (cascaded with
    // it when that record is lost); one without (a root made by hand) counts as based inside
    // unless `meta` records it.
    for other in db::branches(txn)? {
        if other == branch || crate::branch::index::get(txn, &other)?.is_some() {
            continue;
        }
        let base: Option<BranchBase> = db::meta(txn, &format!("branchBase.{other}"))?
            .and_then(|v| serde_json::from_str(&v).ok());
        match base {
            Some(b) if b.branch != branch || b.seq < from_seq => {}
            _ => return Ok(None),
        }
    }
    // (c)
    if lease_top.is_none_or(|top| top < winner_epoch) {
        return Ok(None);
    }
    Ok(Some(Lost {
        from_seq,
        winner_epoch,
        children,
    }))
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

/// What [`take_rows`] took out of a branch, for its [`RewindInfo`].
pub(super) struct Taken {
    pub(super) count: u32,
    pub(super) own: u32,
    writers: BTreeSet<String>,
    epochs: BTreeSet<u64>,
    segments: u32,
    span_ms: i64,
}

impl Taken {
    /// The record of this rewind (`kind`: `appliedLine` or `divergedTail`).
    pub(super) fn info(self, from_rev: u64, kind: &str, winner_epoch: u64, at: i64) -> RewindInfo {
        RewindInfo {
            generation: 0,
            from_rev,
            kind: kind.into(),
            count: self.count,
            own: self.own,
            writers: self.writers.into_iter().collect(),
            epochs: self.epochs.into_iter().collect(),
            winner_epoch,
            segments: self.segments,
            span_ms: self.span_ms,
            at,
            acknowledged: false,
        }
    }
}

/// Takes the rows of `branch` from `from_seq` on out (module docs, "What", steps 1–2):
/// quarantined with the 4A origin rule (exported: `SUPERSEDED` with detail
/// `superseded(seq, epoch)`; unexported: `DIVERGED_TAIL`; `reason`, when given, replaces
/// the first: an unexported row exists nowhere else and stays a diverged tail), then the rows, the snapshots at or above `from_seq` and the `segments` rows reaching
/// it are deleted. `segs` are the branch's folder segments.
pub(super) fn take_rows(
    ctx: &mut Ctx,
    branch: &str,
    from_seq: u64,
    segs: &[SegRef],
    reason: Option<&str>,
    superseded: &dyn Fn(u64, u64) -> String,
) -> Result<Taken, StoreError> {
    let txn = ctx.txn;
    let from = to_i64(from_seq)?;
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
    // A pack (compaction) carries other writers' records under its own `writerId` and the
    // highest epoch it replaces: it names who wrote and at which epoch only when no original
    // segment holding a lost record is left, and never makes a record this server's own.
    let (mut writers, mut pack_writers) = (BTreeSet::new(), BTreeSet::new());
    let (mut epochs, mut pack_epochs) = (BTreeSet::new(), BTreeSet::new());
    let mut ours_in_folder: HashSet<String> = HashSet::new();
    let mut our_ids: HashSet<String> = history::server_ids(txn)?.into_iter().collect();
    our_ids.insert(ctx.server_id().to_string());
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
        if h.pack {
            pack_writers.insert(h.writer_id.clone());
            pack_epochs.insert(h.epoch);
            packs.insert(s.hash().to_string());
            continue;
        }
        writers.insert(h.writer_id.clone());
        epochs.insert(h.epoch);
        plain.insert(s.hash().to_string());
        if our_ids.contains(&h.writer_id) {
            ours_in_folder.extend(held);
        }
    }
    if plain.is_empty() {
        writers = pack_writers;
        epochs = pack_epochs;
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
            superseded(r.seq, r.epoch)
        } else {
            format!("committed at epoch {} as seq {}", r.epoch, r.seq)
        };
        quarantine::upsert(
            txn,
            &Quarantined {
                tx_hash: &r.tx_hash,
                branch,
                origin: if mine { "local" } else { "folder" },
                reason: match reason {
                    Some(why) if r.exported => why,
                    None if r.exported => quarantine::SUPERSEDED,
                    // Exists nowhere else, whatever took the branch out.
                    _ => quarantine::DIVERGED_TAIL,
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

    let committed: Vec<i64> = rows
        .iter()
        .filter(|r| r.exported)
        .map(|r| r.committed_at)
        .collect();
    let span_ms = match (committed.iter().min(), committed.iter().max()) {
        (Some(a), Some(b)) => b - a,
        _ => 0,
    };
    let segments = if plain.is_empty() {
        packs.len()
    } else {
        plain.len()
    };
    Ok(Taken {
        count: u32::try_from(rows.len()).unwrap_or(u32::MAX),
        own,
        writers,
        epochs,
        segments: u32::try_from(segments).unwrap_or(u32::MAX),
        span_ms,
    })
}

/// Takes the lost rows of `branch` out (module docs, "What", steps 1–4). The caller cascades
/// to [`Lost::children`] next.
pub(super) fn run(
    ctx: &mut Ctx,
    branch: &str,
    lost: &Lost,
    segs: &[SegRef],
) -> Result<Rewind, StoreError> {
    let winner = lost.winner_epoch;
    let superseded = |seq: u64, epoch: u64| {
        format!("seq {seq} (epoch {epoch}) superseded by a line at epoch {winner}")
    };
    let taken = take_rows(ctx, branch, lost.from_seq, segs, None, &superseded)?;
    rewind_scan(ctx.txn, branch, lost.from_seq - 1)?;
    let (count, own) = (taken.count, taken.own);
    let info = taken.info(lost.from_seq, "appliedLine", winner, ctx.now);
    let generation = history::record(ctx.txn, branch, info)?;
    Ok(Rewind {
        branch: branch.to_string(),
        from_rev: lost.from_seq,
        quarantined: count,
        own: own > 0,
        generation,
    })
}
