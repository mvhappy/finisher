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
//!    as a commit epoch. A record that cannot be read stops the branch there:
//!    `NEWER_FORMAT` sets `newer`, anything else is the branch's `error`.
//! 4. A `segments` row (`imported`) for each segment touched whose last record is now in
//!    the DB with the same hash, so the next export chains `prevSegmentHash` from it.
//!    Segments only partly in the DB (a fork or a stop inside one) get none.
//! 5. `quarantine` (stale or malformed folder records), keyed by `txHash`, skipping records
//!    the DB holds.

use std::collections::{BTreeSet, HashMap, HashSet};

use fin_core::{CommittedTx, CoreError, apply_committed};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::json;

use super::Verdict;
use super::known::{self, broken};
use super::quarantine::{self, Quarantined};
use crate::chain::{QuarantineReason, Resolution, SegRef};
use crate::counters::rewind_scan;
use crate::db::{self, SNAPSHOT_EVERY, hash_bytes, to_i64, to_u64};
use crate::history::{self, RewindInfo};
use crate::names::{SegmentName, hash12};
use crate::{Head, Rewind, StoreError, refresh};

/// Pass-wide state shared by the branches.
pub(super) struct Ctx<'a> {
    pub(super) txn: &'a Connection,
    server_id: &'a str,
    pub(super) now: i64,
    /// Every DB record, `txId → txHash`, loaded on first use and kept current.
    all: Option<HashMap<String, String>>,
    hashes: HashSet<String>,
    /// `txs` rows were added or removed.
    pub(super) changed: bool,
}

impl<'a> Ctx<'a> {
    pub(super) fn new(txn: &'a Connection, server_id: &'a str, now: i64) -> Self {
        Ctx {
            txn,
            server_id,
            now,
            all: None,
            hashes: HashSet::new(),
            changed: false,
        }
    }

    fn load(&mut self) -> Result<&HashMap<String, String>, StoreError> {
        if self.all.is_none() {
            let mut stmt = self.txn.prepare("SELECT tx_id, tx_hash FROM txs")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let all: HashMap<String, String> = rows.collect::<Result<_, _>>()?;
            self.hashes = all.values().cloned().collect();
            self.all = Some(all);
        }
        Ok(self.all.as_ref().expect("loaded above"))
    }

    pub(super) fn all_tx(&mut self) -> Result<HashMap<String, String>, StoreError> {
        self.load().cloned()
    }

    /// Whether the DB holds a record with this `txHash`.
    pub(super) fn has_hash(&mut self, hash: &str) -> Result<bool, StoreError> {
        self.load()?;
        Ok(self.hashes.contains(hash))
    }

    pub(super) fn server_id(&self) -> &str {
        self.server_id
    }

    fn added(&mut self, tx_id: &str, hash: &str) {
        if let Some(all) = &mut self.all {
            all.insert(tx_id.to_string(), hash.to_string());
            self.hashes.insert(hash.to_string());
        }
        self.changed = true;
    }

    pub(super) fn removed(&mut self, tx_id: &str, hash: &str) {
        if let Some(all) = &mut self.all {
            all.remove(tx_id);
            self.hashes.remove(hash);
        }
        self.changed = true;
    }
}

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

enum Stop {
    Newer,
    Failed(CoreError),
    Db(StoreError),
}

impl From<StoreError> for Stop {
    fn from(e: StoreError) -> Self {
        Stop::Db(e)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    schema_version: u32,
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
        let mut tip_hash = db::tip_hash(txn, branch)?;
        'slices: for slice in &res.apply {
            for (bytes, meta) in slice.records().zip(slice.txs()) {
                if h.rev.checked_add(1) != Some(meta.seq) || meta.parent_hash != tip_hash {
                    return Err(broken(
                        branch,
                        format!(
                            "{} (seq {}) does not continue the DB at seq {}",
                            meta.tx_id, meta.seq, h.rev
                        ),
                    ));
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
                    Err(Stop::Db(e)) => return Err(e),
                }
            }
            touched.push(slice.clone());
        }
        head = Some(h);
    }

    // 4. Segment rows.
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

/// Upcasts, replays and stores one record.
fn one(
    ctx: &mut Ctx,
    branch: &str,
    h: &Head,
    slice: &SegRef,
    bytes: &[u8],
    meta: &crate::chain::RecMeta,
) -> Result<(fin_core::Project, CommittedTx), Stop> {
    let txn = ctx.txn;
    let with = |mut e: CoreError| {
        if let serde_json::Value::Object(map) = &mut e.details {
            map.insert("seq".into(), json!(meta.seq));
            map.insert("segment".into(), json!(slice.hash()));
        }
        e
    };
    let record = match db::parse_record(Some(meta.seq), bytes) {
        Ok(r) => r,
        Err(e) if e.code == "NEWER_FORMAT" => return Err(Stop::Newer),
        Err(e) => return Err(Stop::Failed(with(e))),
    };
    if record.tx_id != meta.tx_id || record.seq != meta.seq || record.intent.branch != branch {
        return Err(Stop::Failed(with(
            CoreError::new(
                "REPLAY_FAILED",
                format!(
                    "record {} at seq {} does not belong to branch {branch:?}",
                    meta.tx_id, meta.seq
                ),
            )
            .with_details(json!({ "txId": meta.tx_id, "branch": branch,
                                  "recordBranch": record.intent.branch })),
        )));
    }
    let applied = apply_committed(&h.project, &record.intent, &record.bindings)
        .map_err(|e| Stop::Failed(with(e)))?;
    let version = serde_json::from_slice::<Version>(bytes)
        .map(|v| v.schema_version)
        .unwrap_or(record.schema_version);
    db::insert_imported(
        txn,
        branch,
        &record,
        bytes,
        &meta.tx_hash,
        version,
        &applied.diff,
        slice.header().epoch,
    )?;
    if meta.seq.is_multiple_of(SNAPSHOT_EVERY) {
        txn.execute(
            "DELETE FROM snapshots WHERE branch = ?1 AND seq = ?2",
            params![branch, to_i64(meta.seq)?],
        )
        .map_err(StoreError::from)?;
        db::insert_snapshot(txn, branch, meta.seq, &applied.project)?;
    }
    ctx.added(&meta.tx_id, &meta.tx_hash);
    let tx = CommittedTx {
        rev: meta.seq,
        tx: record.intent,
        diff: applied.diff,
        bindings: record.bindings,
    };
    Ok((applied.project, tx))
}

/// Records `s` in `segments` if its last record is in the DB with the same hash.
fn record_segment(ctx: &Ctx, branch: &str, s: &SegRef) -> Result<(), StoreError> {
    let seg = s.segment();
    let h = &seg.header;
    let Some(last) = seg.records.last() else {
        return Ok(());
    };
    let stored: Option<String> = ctx
        .txn
        .query_row(
            "SELECT tx_hash FROM txs WHERE branch = ?1 AND seq = ?2",
            params![branch, to_i64(h.last_seq)?],
            |r| r.get(0),
        )
        .ok();
    if stored.as_deref() != Some(hash_bytes(last).as_str()) {
        return Ok(());
    }
    let Some(name) =
        hash12(&seg.hash).and_then(|x| SegmentName::new(h.epoch, h.first_seq, h.last_seq, x))
    else {
        return Ok(());
    };
    let state = if h.writer_id == ctx.server_id {
        "exported"
    } else {
        "imported"
    };
    ctx.txn.execute(
        "INSERT INTO segments (branch, hash, name, epoch, first_seq, last_seq, pack, replaces, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(hash) DO NOTHING",
        params![
            branch,
            seg.hash,
            name.to_string(),
            to_i64(h.epoch)?,
            to_i64(h.first_seq)?,
            to_i64(h.last_seq)?,
            h.pack,
            serde_json::to_string(&h.replaces)?,
            state
        ],
    )?;
    Ok(())
}
