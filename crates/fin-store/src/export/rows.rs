//! The `txs` and `segments` rows export reads and writes.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use fin_core::CoreError;

use super::Attempt;
use crate::StoreError;
use crate::db::{to_i64, to_u64};
use crate::segment;

/// An unexported `txs` row.
pub(super) struct Row {
    pub(super) seq: u64,
    pub(super) bytes: Vec<u8>,
    pub(super) tx_hash: String,
    pub(super) schema_version: u32,
}

/// Branches with unexported records.
pub(super) fn pending_branches(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT branch FROM txs WHERE exported = 0 ORDER BY branch")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The next (at most [`segment::MAX_RECORDS`]) unexported rows of `branch`, in seq order,
/// after checking that the exported rows are a seq prefix and the rows are consecutive.
pub(super) fn pending_rows(conn: &Connection, branch: &str) -> Result<Vec<Row>, StoreError> {
    let (max_exported, min_pending): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT (SELECT MAX(seq) FROM txs WHERE branch = ?1 AND exported = 1),
                (SELECT MIN(seq) FROM txs WHERE branch = ?1 AND exported = 0)",
        params![branch],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let Some(min_pending) = min_pending else {
        return Ok(Vec::new());
    };
    if max_exported.is_some_and(|m| m + 1 != min_pending) {
        return Err(broken(
            branch,
            format!(
                "exported records are not a seq prefix (exported through {max_exported:?}, first unexported {min_pending})"
            ),
        ));
    }
    let mut stmt = conn.prepare(
        "SELECT seq, record_bytes, tx_hash, schema_version FROM txs
         WHERE branch = ?1 AND exported = 0 ORDER BY seq LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![branch, segment::MAX_RECORDS as i64], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, u32>(3)?,
        ))
    })?;
    let mut out: Vec<Row> = Vec::new();
    for row in rows {
        let (seq, bytes, tx_hash, schema_version) = row?;
        let seq = to_u64(seq)?;
        if out.last().is_some_and(|p| p.seq + 1 != seq) {
            return Err(broken(
                branch,
                format!("seq {seq} does not follow the one before"),
            ));
        }
        out.push(Row {
            seq,
            bytes,
            tx_hash,
            schema_version,
        });
    }
    Ok(out)
}

pub(super) fn broken(branch: &str, why: String) -> StoreError {
    CoreError::new(
        "DB_ERROR",
        format!("cannot export branch {branch:?}: {why}"),
    )
    .with_details(json!({ "branch": branch }))
    .into()
}

/// The hash of the segment whose last record is the one before `first`; `None` when no record
/// precedes it on the branch.
pub(super) fn prev_segment(
    conn: &Connection,
    branch: &str,
    first: u64,
) -> Result<Option<String>, StoreError> {
    let before: i64 = conn.query_row(
        "SELECT COUNT(*) FROM txs WHERE branch = ?1 AND seq < ?2",
        params![branch, to_i64(first)?],
        |r| r.get(0),
    )?;
    if before == 0 {
        return Ok(None);
    }
    let hash: Option<String> = conn
        .query_row(
            "SELECT hash FROM segments WHERE branch = ?1 AND last_seq = ?2
             AND state IN ('exported', 'imported') ORDER BY pack DESC, hash LIMIT 1",
            params![branch, to_i64(first - 1)?],
            |r| r.get(0),
        )
        .optional()?;
    match hash {
        Some(h) => Ok(Some(h)),
        None => Err(broken(
            branch,
            format!("no known segment ends at seq {}", first - 1),
        )),
    }
}

/// Marks the segment's rows exported and records the segment, in one DB transaction.
pub(super) fn mark_exported(
    conn: &mut Connection,
    branch: &str,
    a: &Attempt,
) -> Result<u32, StoreError> {
    let txn = conn.transaction()?;
    let expected = a.last_seq - a.first_seq + 1;
    let changed = txn.execute(
        "UPDATE txs SET exported = 1 WHERE branch = ?1 AND seq BETWEEN ?2 AND ?3 AND exported = 0",
        params![branch, to_i64(a.first_seq)?, to_i64(a.last_seq)?],
    )?;
    if changed as u64 != expected {
        return Err(broken(
            branch,
            format!(
                "{changed} of {expected} records at seqs {}..={} were unexported",
                a.first_seq, a.last_seq
            ),
        ));
    }
    txn.execute(
        "INSERT INTO segments (branch, hash, name, epoch, first_seq, last_seq, pack, replaces, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, '[]', 'exported') ON CONFLICT(hash) DO NOTHING",
        params![
            branch,
            a.hash,
            a.name,
            to_i64(a.epoch)?,
            to_i64(a.first_seq)?,
            to_i64(a.last_seq)?
        ],
    )?;
    txn.commit()?;
    Ok(u32::try_from(expected).unwrap_or(u32::MAX))
}
