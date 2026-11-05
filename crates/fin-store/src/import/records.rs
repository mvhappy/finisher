//! Reading and storing single records for [`super::apply`]: the stop reasons, seed lookup and
//! the per-record steps.

use std::collections::HashMap;

use fin_core::{CommittedTx, CoreError, apply_committed};
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use super::ctx::Ctx;
use super::known::broken;

use crate::chain::{Resolution, SegRef};
use crate::db::{self, SNAPSHOT_EVERY, hash_bytes, to_i64};

use crate::names::{SegmentName, hash12};
use crate::{Head, StoreError};

pub(super) enum Stop {
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
pub(super) struct Version {
    pub(super) schema_version: u32,
}

/// The highest seed of `branch` that this resolution reaches from seq 1 and whose records up
/// to it all read as this branch's (module docs, step 3).
pub(super) fn seed_for<'s>(
    ctx: &Ctx<'s>,
    branch: &str,
    res: &Resolution,
) -> Option<&'s super::Seed> {
    let mut on_chain = HashMap::new();
    for slice in &res.apply {
        for (bytes, meta) in slice.records().zip(slice.txs()) {
            on_chain.insert(meta.seq, (meta.tx_hash.as_str(), bytes, meta, slice));
        }
    }
    let readable = |through: u64| {
        (1..=through).all(|seq| {
            on_chain
                .get(&seq)
                .is_some_and(|(_, bytes, meta, slice)| read(branch, slice, bytes, meta).is_ok())
        })
    };
    ctx.seeds
        .iter()
        .filter(|s| s.branch == branch && s.seq > 0)
        .filter(|s| on_chain.get(&s.seq).is_some_and(|(h, ..)| *h == s.tx_hash))
        .max_by_key(|s| s.seq)
        .filter(|s| readable(s.seq))
}

/// Stores one record of a seeded range: read, not replayed (no diff yet).
pub(super) fn unreplayed(
    ctx: &mut Ctx,
    branch: &str,
    slice: &SegRef,
    bytes: &[u8],
    meta: &crate::chain::RecMeta,
) -> Result<(), StoreError> {
    let record = match read(branch, slice, bytes, meta) {
        Ok(r) => r,
        Err(Stop::Db(e)) => return Err(e),
        Err(_) => {
            return Err(broken(
                branch,
                format!("seeded record {} does not read", meta.tx_id),
            ));
        }
    };
    let version = serde_json::from_slice::<Version>(bytes)
        .map(|v| v.schema_version)
        .unwrap_or(record.schema_version);
    db::insert_imported(
        ctx.txn,
        branch,
        &record,
        bytes,
        &meta.tx_hash,
        version,
        None,
        slice.header().epoch,
    )?;
    ctx.added(&meta.tx_id, &meta.tx_hash);
    Ok(())
}

/// Upcasts one record and checks it is the one the chain says, on `branch`.
pub(super) fn read(
    branch: &str,
    slice: &SegRef,
    bytes: &[u8],
    meta: &crate::chain::RecMeta,
) -> Result<fin_core::Record, Stop> {
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
    Ok(record)
}

/// Upcasts, replays and stores one record.
pub(super) fn one(
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
    let record = read(branch, slice, bytes, meta)?;
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
        Some(&applied.diff),
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
pub(super) fn record_segment(ctx: &Ctx, branch: &str, s: &SegRef) -> Result<(), StoreError> {
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
    // A pack carries records other servers may have written: never this server's own.
    let state = if h.writer_id == ctx.server_id && !h.pack {
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
