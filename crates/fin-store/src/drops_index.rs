//! The `drops` index (plan 5E Decision 4; sub-project 5 spec §8.3): one row per origin a
//! `drop` record names, `(tx_id, branch, seq, origin, dropped)`, so what is dropped for a
//! branch ([`fin_core::history::dropped_for`]) is read along its lineage's bounds without
//! parsing records.
//!
//! Derived from `txs` like `record_links`: every append (a write, an import, a rebuild's
//! re-import) adds a record's rows in the same transaction ([`insert`]); after a row deletion
//! (a rewind, a diverged tail, a cascade) [`forget_gone`] drops the rows of records that are
//! gone (`crate::derived::recompute`); verify compares the table with the records
//! ([`drift`]). [`marks_parsed`] reads the same marks from the records themselves, for the
//! derived state's reference computation (`crate::derived::compute`), which must follow from
//! the log alone.

use std::collections::BTreeSet;

use fin_core::history::{DropMark, Lineage};
use fin_core::{Link, Record, RecordKind};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::StoreError;
use crate::db::{self, to_i64, to_u64};

/// One `drops` row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DropRow {
    pub(crate) tx_id: String,
    pub(crate) branch: String,
    pub(crate) seq: i64,
    pub(crate) origin: String,
    pub(crate) dropped: bool,
}

/// The rows `record` on `branch` gets: one per origin of a `drop` record (`drops`, then
/// `undrops`), none for any other kind. A `drop` record whose fields do not decode is
/// `INVALID_FORMAT` (never written: the write path builds it through `Link::new_drop`).
pub(crate) fn rows_of(branch: &str, record: &Record) -> Result<Vec<DropRow>, StoreError> {
    if record.kind != RecordKind::Drop {
        return Ok(Vec::new());
    }
    let Link::Drop { drops, undrops, .. } = record.link()? else {
        return Ok(Vec::new());
    };
    let seq = to_i64(record.seq)?;
    let row = |origin: String, dropped: bool| DropRow {
        tx_id: record.tx_id.clone(),
        branch: branch.to_string(),
        seq,
        origin,
        dropped,
    };
    let mut out: Vec<DropRow> = drops.into_iter().map(|o| row(o, true)).collect();
    out.extend(undrops.into_iter().map(|o| row(o, false)));
    Ok(out)
}

/// Adds the rows of `record`, just appended on `branch` ([`rows_of`]).
pub(crate) fn insert(conn: &Connection, branch: &str, record: &Record) -> Result<(), StoreError> {
    let rows = rows_of(branch, record)?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare_cached(
        "INSERT INTO drops (tx_id, branch, seq, origin, dropped) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for r in rows {
        stmt.execute(params![r.tx_id, r.branch, r.seq, r.origin, r.dropped])?;
    }
    Ok(())
}

/// Drops the rows whose record is gone (after a row deletion; `crate::derived::recompute`).
pub(crate) fn forget_gone(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM drops WHERE tx_id NOT IN (SELECT tx_id FROM txs)",
        [],
    )?;
    Ok(())
}

/// `(branch, after, through)` of each lineage segment in lineage order (the furthest
/// ancestor first), `through` as an `i64` bound.
fn bounds(lineage: &Lineage) -> Result<Vec<(&str, i64, i64)>, StoreError> {
    lineage
        .segs
        .iter()
        .rev()
        .map(|(b, after, through)| {
            Ok((
                b.as_str(),
                to_i64(*after)?,
                through.map_or(Ok(i64::MAX), to_i64)?,
            ))
        })
        .collect()
}

/// The marks of the `drop` records on `lineage`, in lineage order, from the index: what
/// `fin_core::history::drops::marks_from_records` reads from the records.
pub(crate) fn marks(conn: &Connection, lineage: &Lineage) -> Result<Vec<DropMark>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, origin, dropped FROM drops WHERE branch = ?1 AND seq > ?2 AND seq <= ?3
         ORDER BY seq, dropped DESC, origin",
    )?;
    let mut out = Vec::new();
    for (branch, after, through) in bounds(lineage)? {
        let rows: Vec<(i64, String, bool)> = stmt
            .query_map(params![branch, after, through], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<Result<_, _>>()?;
        for (seq, origin, dropped) in rows {
            out.push(DropMark {
                branch: branch.to_string(),
                seq: to_u64(seq)?,
                origin,
                dropped,
            });
        }
    }
    Ok(out)
}

/// [`marks`] from the `drop` records themselves (only those rows are parsed).
pub(crate) fn marks_parsed(
    conn: &Connection,
    lineage: &Lineage,
) -> Result<Vec<DropMark>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, record_bytes FROM txs
         WHERE branch = ?1 AND seq > ?2 AND seq <= ?3 AND kind = 'drop' ORDER BY seq",
    )?;
    let mut out = Vec::new();
    for (branch, after, through) in bounds(lineage)? {
        let rows: Vec<(i64, Vec<u8>)> = stmt
            .query_map(params![branch, after, through], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        for (seq, bytes) in rows {
            let record = db::parse_record(Some(to_u64(seq)?), &bytes)?;
            for r in rows_of(branch, &record)? {
                out.push(DropMark {
                    branch: r.branch,
                    seq: to_u64(r.seq)?,
                    origin: r.origin,
                    dropped: r.dropped,
                });
            }
        }
    }
    Ok(out)
}

/// Verify's check: the `drops` rows against the rows the `drop` records give, as
/// `{fromLog, stored}` pairs (at most `limit`), and how many differ.
pub(crate) fn drift(conn: &Connection, limit: usize) -> Result<(Vec<Value>, u64), StoreError> {
    let mut want = BTreeSet::new();
    let mut stmt = conn.prepare("SELECT branch, seq, record_bytes FROM txs WHERE kind = 'drop'")?;
    let rows: Vec<(String, i64, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    for (branch, seq, bytes) in rows {
        let record = db::parse_record(Some(to_u64(seq)?), &bytes)?;
        want.extend(rows_of(&branch, &record)?);
    }
    let mut stmt = conn.prepare("SELECT tx_id, branch, seq, origin, dropped FROM drops")?;
    let have: BTreeSet<DropRow> = stmt
        .query_map([], |r| {
            Ok(DropRow {
                tx_id: r.get(0)?,
                branch: r.get(1)?,
                seq: r.get(2)?,
                origin: r.get(3)?,
                dropped: r.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut out = Vec::new();
    let mut count = 0u64;
    let mut push = |v: Value| {
        count += 1;
        if out.len() < limit {
            out.push(v);
        }
    };
    for w in want.difference(&have) {
        push(json!({ "fromLog": w, "stored": null }));
    }
    for h in have.difference(&want) {
        push(json!({ "fromLog": null, "stored": h }));
    }
    Ok((out, count))
}
