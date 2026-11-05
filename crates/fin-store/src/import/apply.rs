//! Acting on one branch's [`Resolution`], inside the pass's DB transaction.
//!
//! Order:
//! 1. `mark_exported`: tail rows the folder already holds (a crash between the rename and
//!    the marking) become exported, in seq order, so the exported rows stay a prefix. The
//!    file's `segments` row is recorded (`exported` when this server wrote it, else
//!    `imported`).
//! 2. `local_tail_diverged`: every remaining unexported row moves to quarantine
//!    (`DIVERGED_TAIL`, origin `local`), its rows and any DB snapshot inside it are deleted,
//!    and the head is rebuilt from the latest DB snapshot plus the remaining rows. This comes
//!    before step 3: a retried transaction that the folder holds under the same `txId` (the
//!    walk exempts it from `TxIdReused`) can then take that `txId`. The id counters are not
//!    touched: ids the tail allocated are never handed out again.
//! 3. `apply`: each record is upcast and replayed with [`apply_committed`] (structural checks
//!    only), then stored with its exact bytes and hash, already exported. `txs.epoch` is the
//!    epoch of the segment it came from: a record does not carry its commit epoch, and a
//!    segment is labelled with the highest commit epoch of its records (export), so this is
//!    an upper bound. Only unexported rows' epochs are ever used (export labels, the tail's
//!    epoch in the epoch rule), and imported rows are exported, so the bound is never used
//!    as a commit epoch (`super::aside` reads it as the label it is, for a `branch` record
//!    whose id a set-aside record also made). A record that cannot be read stops the branch there:
//!    `NEWER_FORMAT` sets `newer`, anything else is the branch's `error`.
//!    **Branch records** (plan 4B Decisions 14–15): a `branch` or `branchSet` record is folded
//!    into the index before its row is stored (`crate::branch::index::absorb_imported`); a
//!    `branch` record also stores its child's fork state. One that does not fit the index (a
//!    name the rules refuse, an id already known, a `forkHash` that is not the parent's
//!    record) is set aside as `MALFORMED` with every record after it on the resolved chain,
//!    and the pass goes on: no error, nothing applied past it.
//!    **Seeding** (a rebuild only, `crate::rebuild`): on a branch whose DB holds only its start
//!    (rev 0 on `main`; the fork snapshot on a 4B branch),
//!    a folder snapshot ([`super::Seed`]) whose `(seq, txHash)` is a record of this
//!    resolution (so on the resolved chain, from the branch's start) and whose records up to
//!    it all read as this branch's, none of them a `branch` or `branchSet` record, is the state
//!    after that seq: those records are stored without being
//!    replayed (their diffs are filled later, `crate::diffs`), and replay continues from the
//!    snapshot. The highest such snapshot wins; any other is ignored.
//! 4. A `segments` row (`imported`) for each segment touched whose last record is now in
//!    the DB with the same hash, so the next export chains `prevSegmentHash` from it.
//!    Segments only partly in the DB (a fork or a stop inside one) get none. Every present
//!    pack ending on the DB line gets one too, touched or not, unless the resolution sets
//!    any of it aside (a malformed, lying pack): once compaction deletes the segments it
//!    replaces, the pack is what the DB knows the folder by (project copies,
//!    `crate::copies`).
//! 5. `quarantine` (stale or malformed folder records), keyed by `txHash`, skipping records
//!    the DB holds.

use std::collections::{BTreeSet, HashMap, HashSet};

use fin_core::{CommittedTx, CoreError};
use rusqlite::params;

use super::Verdict;
use super::ctx::Ctx;
use super::known::{self, broken};
use super::quarantine::{self, Quarantined};
use super::records::{Stop, one, record_segment, seed_for, unreplayed};
use crate::chain::{QuarantineReason, Resolution, SegRef};
use crate::counters::rewind_scan;
use crate::db::{self, hash_bytes, to_i64, to_u64};
use crate::history::{self, RewindInfo};
use crate::{Head, Rewind, StoreError, refresh};

pub(super) struct Outcome {
    pub(super) applied: Vec<CommittedTx>,
    /// The branch head after this branch's changes.
    pub(super) head: Option<Head>,
    /// `txs` rows of this branch changed.
    pub(super) changed: bool,
    pub(super) verdict: Verdict,
    /// The diverged tail that was set aside.
    pub(super) rewound: Option<Rewind>,
}

/// Acts on `res` for `branch` (module docs). `head` is the branch head as committed.
pub(super) fn branch(
    ctx: &mut Ctx,
    branch: &str,
    line: &known::Line,
    segs: &[SegRef],
    res: &Resolution,
    head: Option<Head>,
) -> Result<Outcome, StoreError> {
    let txn = ctx.txn;
    let mut verdict = Verdict {
        fork: res.fork.clone(),
        waiting: res.waiting.clone(),
        pending: res.pending.iter().map(|s| s.hash().to_string()).collect(),
        ..Verdict::default()
    };
    let mut touched: Vec<SegRef> = vec![];
    let mut changed = false;
    let find = |hash: &str| segs.iter().find(|s| s.hash() == hash).cloned();

    // 1. Mark exported.
    for (i, (tx_id, seg)) in res.mark_exported.iter().enumerate() {
        let row = line
            .tail
            .get(i)
            .filter(|t| t.tx_id == *tx_id)
            .ok_or_else(|| {
                broken(
                    branch,
                    format!("{tx_id} to mark exported is not the next tail record"),
                )
            })?;
        let n = txn.execute(
            "UPDATE txs SET exported = 1 WHERE branch = ?1 AND seq = ?2 AND tx_id = ?3 AND exported = 0",
            params![branch, to_i64(row.seq)?, tx_id],
        )?;
        if n != 1 {
            return Err(broken(branch, format!("{tx_id} is not an unexported row")));
        }
        touched.extend(find(seg));
        changed = true;
    }

    // 2. A diverged tail moves to quarantine.
    let mut head = head;
    let mut rewound = None;
    if res.local_tail_diverged {
        let mut stmt = txn.prepare(
            "SELECT seq, tx_id, tx_hash, record_bytes, epoch FROM txs
             WHERE branch = ?1 AND exported = 0 ORDER BY seq",
        )?;
        let rows: Vec<(i64, String, String, Vec<u8>, i64)> = stmt
            .query_map(params![branch], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<Result<_, _>>()?;
        if let Some(first) = rows.first().map(|r| r.0) {
            let mut epochs = BTreeSet::new();
            for (seq, tx_id, tx_hash, bytes, epoch) in &rows {
                epochs.insert(to_u64(*epoch)?);
                quarantine::upsert(
                    txn,
                    &Quarantined {
                        tx_hash,
                        branch,
                        origin: "local",
                        reason: quarantine::DIVERGED_TAIL,
                        detail: &format!("committed at epoch {epoch} as seq {seq}"),
                        bytes,
                        now: ctx.now,
                    },
                )?;
                ctx.removed(tx_id, tx_hash);
            }
            txn.execute(
                "DELETE FROM txs WHERE branch = ?1 AND exported = 0",
                params![branch],
            )?;
            txn.execute(
                "DELETE FROM snapshots WHERE branch = ?1 AND seq >= ?2",
                params![branch, first],
            )?;
            let tip = to_u64(first)?.saturating_sub(1);
            rewind_scan(txn, branch, tip)?;
            let count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
            let info = RewindInfo {
                generation: 0,
                from_rev: tip + 1,
                kind: "divergedTail".into(),
                count,
                own: count,
                writers: vec![ctx.server_id.to_string()],
                epochs: epochs.into_iter().collect(),
                winner_epoch: 0,
                segments: 0,
                span_ms: 0,
                at: ctx.now,
                acknowledged: false,
            };
            let generation = history::record(txn, branch, info)?;
            rewound = Some(Rewind {
                branch: branch.to_string(),
                from_rev: tip + 1,
                quarantined: count,
                own: true,
                generation,
            });
            let mut fresh = HashMap::new();
            refresh(txn, &mut fresh, branch)?;
            head = fresh.remove(branch);
            changed = true;
        }
    }

    // 3. Apply.
    let mut applied = vec![];
    if !res.apply.is_empty() {
        let mut h = head
            .clone()
            .ok_or_else(|| broken(branch, "the branch has no head to import onto".into()))?;
        let mut tip_hash = crate::branch::lineage::tip_hash(txn, branch)?;
        // A branch's start: 0 on `main`, the fork seq on a 4B branch (its head before its
        // first record is the fork state, Decision 14, never a project genesis).
        let start = crate::branch::index::get(txn, branch)?.map_or(0, |i| i.fork_seq);
        let seed = if h.rev == start {
            seed_for(ctx, branch, start, res)
        } else {
            None
        };
        let mut aside_from: Option<(usize, usize, CoreError)> = None;
        'slices: for (si, slice) in res.apply.iter().enumerate() {
            for (ri, (bytes, meta)) in slice.records().zip(slice.txs()).enumerate() {
                if h.rev.checked_add(1) != Some(meta.seq) || meta.parent_hash != tip_hash {
                    return Err(broken(
                        branch,
                        format!(
                            "{} (seq {}) does not continue the DB at seq {}",
                            meta.tx_id, meta.seq, h.rev
                        ),
                    ));
                }
                if let Some(seed) = seed
                    && meta.seq <= seed.seq
                {
                    unreplayed(ctx, branch, slice, bytes, meta)?;
                    h.rev = meta.seq;
                    h.tip = Some(meta.tx_hash.clone());
                    tip_hash = Some(meta.tx_hash.clone());
                    if meta.seq == seed.seq {
                        h.project = seed.project.clone();
                        txn.execute(
                            "DELETE FROM snapshots WHERE branch = ?1 AND seq = ?2",
                            params![branch, to_i64(seed.seq)?],
                        )?;
                        db::insert_snapshot(txn, branch, seed.seq, &h.project)?;
                        crate::diffs::set_pending(txn, branch, seed.seq)?;
                        ctx.seeded = Some(seed.seq);
                    }
                    changed = true;
                    continue;
                }
                match one(ctx, branch, &h, slice, bytes, meta) {
                    Ok((project, tx)) => {
                        h = Head {
                            rev: meta.seq,
                            tip: Some(meta.tx_hash.clone()),
                            project,
                        };
                        tip_hash = Some(meta.tx_hash.clone());
                        applied.push(tx);
                        changed = true;
                    }
                    Err(Stop::Newer) => {
                        verdict.newer = true;
                        break 'slices;
                    }
                    Err(Stop::Failed(e)) => {
                        verdict.error = Some(e);
                        break 'slices;
                    }
                    Err(Stop::Aside(e)) => {
                        aside_from = Some((si, ri, e));
                        break 'slices;
                    }
                    Err(Stop::Db(e)) => return Err(e),
                }
            }
            touched.push(slice.clone());
        }
        head = Some(h);
        if let Some((si, ri, e)) = aside_from {
            set_aside_rest(ctx, branch, res, si, ri, &e, &mut verdict)?;
        }
    }

    // 4. Segment rows (and every pack that ends on the DB line, unless this resolution sets
    //    any of it aside: a lying pack must never be chained from).
    let aside: HashSet<&str> = res.quarantine.iter().map(|(s, _)| s.hash()).collect();
    touched.extend(
        segs.iter()
            .filter(|s| s.header().pack && !aside.contains(s.hash()))
            .cloned(),
    );
    touched.sort_by(|a, b| a.hash().cmp(b.hash()));
    touched.dedup_by(|a, b| a.hash() == b.hash());
    for s in &touched {
        record_segment(ctx, branch, s)?;
    }

    // 5. Folder records set aside.
    for (slice, why) in &res.quarantine {
        let (reason, detail) = match why {
            QuarantineReason::StaleEpoch => (quarantine::STALE_EPOCH, slice.hash().to_string()),
            QuarantineReason::Malformed(m) => {
                (quarantine::MALFORMED, format!("{}: {m}", slice.hash()))
            }
        };
        for bytes in slice.records() {
            let tx_hash = hash_bytes(bytes);
            if ctx.has_hash(&tx_hash)? {
                continue;
            }
            quarantine::upsert(
                txn,
                &Quarantined {
                    tx_hash: &tx_hash,
                    branch,
                    origin: "folder",
                    reason,
                    detail: &detail,
                    bytes,
                    now: ctx.now,
                },
            )?;
            verdict.quarantined.insert(tx_hash);
        }
    }
    Ok(Outcome {
        applied,
        head,
        changed,
        verdict,
        rewound,
    })
}

/// A record of the resolved chain that does not fit the branch index (`records::Stop::Aside`)
/// is set aside as `MALFORMED`, and every record after it on the chain with it (they descend
/// from it): nothing is applied past it, and the pass goes on.
fn set_aside_rest(
    ctx: &mut Ctx,
    branch: &str,
    res: &Resolution,
    si: usize,
    ri: usize,
    e: &CoreError,
    verdict: &mut Verdict,
) -> Result<(), StoreError> {
    let first = &res.apply[si];
    let first_hash = first.txs()[ri].tx_hash.clone();
    let short = crate::names::hash12(&first_hash)
        .unwrap_or(&first_hash)
        .to_string();
    for (i, slice) in res.apply.iter().enumerate().skip(si) {
        let skip = if i == si { ri } else { 0 };
        for (bytes, meta) in slice.records().zip(slice.txs()).skip(skip) {
            if ctx.has_hash(&meta.tx_hash)? {
                continue;
            }
            let detail = if meta.tx_hash == first_hash {
                format!("{}: {}", slice.hash(), e.message)
            } else {
                format!("{}: descends from set-aside record {short}", slice.hash())
            };
            quarantine::upsert(
                ctx.txn,
                &Quarantined {
                    tx_hash: &meta.tx_hash,
                    branch,
                    origin: "folder",
                    reason: quarantine::MALFORMED,
                    detail: &detail,
                    bytes,
                    now: ctx.now,
                },
            )?;
            verdict.quarantined.insert(meta.tx_hash.clone());
        }
    }
    ctx.aside = None;
    Ok(())
}
