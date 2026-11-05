//! Folder branch directories whose `branch` record is set aside (plan 4B Decision 8).
//!
//! A `branch` record that is a current quarantine row (`STALE_EPOCH`, `SUPERSEDED`,
//! `DIVERGED_TAIL`, `MALFORMED`) takes its branch's folder segments with it:
//! - [`leftovers`]: a directory `segments/br<n>/` the pass never reached (no branch the DB
//!   knows) whose `branch` record is set aside has every record quarantined with that row's
//!   reason (origin `folder`, detail `branch record <hash12> set aside`), recursively for
//!   the grandchildren its records make; a directory whose branch record is unknown waits
//!   (`ImportReport::unknown_branches`, never read-only).
//! - [`collisions`]: the same id made twice, on two servers across a takeover (one `branch`
//!   record set aside at the older epoch, one live): the directory holds both branches'
//!   segments. Every record of the live branch was committed after its `branch` record, so
//!   at or above that record's epoch; a segment below it belongs to the set-aside branch and
//!   is quarantined as above, before resolution (without it, a set-aside child forked at
//!   the same point as the live one would be applied onto it). Limit: the live record's
//!   `txs.epoch` is its segment's label, an upper bound of its commit epoch; a holder that
//!   re-acquired the lease with that record still unexported, and then made the same id
//!   collide, could see its own older child segments set aside (recoverable, Task 16).

use std::collections::{BTreeSet, HashMap};

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;

use super::ctx::Ctx;
use super::quarantine::{self, Quarantined};
use crate::StoreError;
use crate::chain::SegRef;
use crate::db::{hash_bytes, to_i64, to_u64};
use crate::names::hash12;

/// Branch id → its set-aside `branch` records `(txHash, reason)`, by `txHash`.
pub(super) type AsideMap = HashMap<String, Vec<(String, String)>>;

/// The reasons that take a branch's segments along (Decision 8).
const REASONS: [&str; 4] = [
    quarantine::STALE_EPOCH,
    quarantine::SUPERSEDED,
    quarantine::DIVERGED_TAIL,
    quarantine::MALFORMED,
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchHead {
    kind: String,
    branch_id: Option<String>,
}

/// The branch id a `branch` record's bytes make, if they are one.
fn branch_id_of(bytes: &[u8]) -> Option<String> {
    let h: BranchHead = serde_json::from_slice(bytes).ok()?;
    (h.kind == "branch").then_some(h.branch_id).flatten()
}

/// The set-aside `branch` records that `current` accepts (`(txHash, reason)`), minus records
/// the DB holds.
fn scan(ctx: &mut Ctx, current: &dyn Fn(&str, &str) -> bool) -> Result<AsideMap, StoreError> {
    let rows: Vec<(String, String, Vec<u8>)> = {
        let mut stmt = ctx.txn.prepare(
            "SELECT tx_hash, reason, record_bytes FROM quarantine
             WHERE reason IN (?1, ?2, ?3, ?4)
               AND CAST(record_bytes AS TEXT) LIKE '%\"kind\":\"branch\"%'
             ORDER BY tx_hash",
        )?;
        stmt.query_map(
            params![REASONS[0], REASONS[1], REASONS[2], REASONS[3]],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
        .collect::<Result<_, _>>()?
    };
    let mut out = AsideMap::new();
    for (hash, reason, bytes) in rows {
        let Some(id) = branch_id_of(&bytes) else {
            continue;
        };
        if !current(&hash, &reason) || ctx.has_hash(&hash)? {
            continue;
        }
        out.entry(id).or_default().push((hash, reason));
    }
    Ok(out)
}

/// The set-aside `branch` records of `branch` during the worklist: rows current now (this
/// pass's verdicts so far have upserted theirs; a row the pass will clear still counts, for
/// one pass). Cached in `ctx` until more records are set aside.
pub(super) fn records_for(
    ctx: &mut Ctx,
    branch: &str,
) -> Result<Vec<(String, String)>, StoreError> {
    if ctx.aside.is_none() {
        let txn = ctx.txn;
        let is_current = |hash: &str, _reason: &str| {
            txn.query_row(
                "SELECT current FROM quarantine WHERE tx_hash = ?1",
                params![hash],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .ok()
            .flatten()
                == Some(1)
        };
        ctx.aside = Some(scan(ctx, &is_current)?);
    }
    Ok(ctx
        .aside
        .as_ref()
        .and_then(|m| m.get(branch).cloned())
        .unwrap_or_default())
}

/// Quarantines every record of `segs` the DB does not hold as `reason`, for the set-aside
/// branch record `record`; returns their `txHash`es.
fn take(
    ctx: &mut Ctx,
    branch: &str,
    segs: &[SegRef],
    record: &str,
    reason: &str,
) -> Result<BTreeSet<String>, StoreError> {
    let detail = format!(
        "branch record {} set aside",
        hash12(record).unwrap_or(record)
    );
    let mut out = BTreeSet::new();
    for s in segs {
        for bytes in s.records() {
            let tx_hash = hash_bytes(bytes);
            if ctx.has_hash(&tx_hash)? || !out.insert(tx_hash.clone()) {
                continue;
            }
            quarantine::upsert(
                ctx.txn,
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
        }
    }
    if !out.is_empty() {
        ctx.aside = None;
    }
    Ok(out)
}

/// The live branch `branch`'s segments below the epoch of its own `branch` record, when a
/// set-aside record made the same id (module docs): quarantined, and taken out of `segs`.
pub(super) fn collisions(
    ctx: &mut Ctx,
    branch: &str,
    segs: Vec<SegRef>,
    aside: &[(String, String)],
) -> Result<(Vec<SegRef>, BTreeSet<String>), StoreError> {
    let Some((record, reason)) = aside.first() else {
        return Ok((segs, BTreeSet::new()));
    };
    let Some(info) = crate::branch::index::get(ctx.txn, branch)? else {
        return Ok((segs, BTreeSet::new()));
    };
    let (Some(parent), Some(created)) = (info.parent.as_deref(), info.created_seq) else {
        return Ok((segs, BTreeSet::new()));
    };
    let epoch: Option<i64> = ctx
        .txn
        .query_row(
            "SELECT epoch FROM txs WHERE branch = ?1 AND seq = ?2",
            params![parent, to_i64(created)?],
            |r| r.get(0),
        )
        .optional()?;
    let Some(epoch) = epoch.map(to_u64).transpose()? else {
        return Ok((segs, BTreeSet::new()));
    };
    let (below, keep): (Vec<SegRef>, Vec<SegRef>) =
        segs.into_iter().partition(|s| s.header().epoch < epoch);
    let taken = take(ctx, branch, &below, record, reason)?;
    Ok((keep, taken))
}

/// Classifies the folder directories the worklist never reached (module docs): returns the
/// records set aside and the directories that wait. `current` holds this pass's verdicts
/// (every branch's and the foreign files'); `DIVERGED_TAIL` rows are current until their
/// record is in `txs`.
pub(super) fn leftovers(
    ctx: &mut Ctx,
    mut dirs: HashMap<String, Vec<SegRef>>,
    current: &BTreeSet<String>,
) -> Result<(BTreeSet<String>, Vec<String>), StoreError> {
    let mut taken = BTreeSet::new();
    loop {
        let map = {
            let is_current = |hash: &str, reason: &str| {
                reason == quarantine::DIVERGED_TAIL
                    || current.contains(hash)
                    || taken.contains(hash)
            };
            scan(ctx, &is_current)?
        };
        let mut names: Vec<String> = dirs.keys().cloned().collect();
        names.sort();
        let mut progress = false;
        for name in names {
            let Some((record, reason)) = map.get(&name).and_then(|v| v.first()) else {
                continue;
            };
            let segs = dirs.remove(&name).unwrap_or_default();
            taken.extend(take(ctx, &name, &segs, record, reason)?);
            progress = true;
        }
        if !progress {
            break;
        }
    }
    let mut unknown: Vec<String> = dirs.into_keys().collect();
    unknown.sort();
    Ok((taken, unknown))
}
