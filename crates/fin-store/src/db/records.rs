//! Row access for `txs`: building, inserting, reading and replaying commit records.

use std::collections::BTreeMap;

use fin_core::{
    Applied, CommittedTx, CoreError, Diff, Link, Op, Project, Record, RecordKind, Tx,
    apply_committed, encode_record, resolve_ops, upcast_diff, upcast_record,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::Value;

use super::{hash_bytes, to_i64, to_u64};
use crate::StoreError;

/// The record a commit of `intent` at `seq` stores (spec §3.3), of kind `link`. For an edit
/// these are the bytes migration 0001 wrote for sub-project 2 history
/// (`migration_0001_matches_encode_record`).
pub(crate) fn new_record(
    seq: u64,
    link: Link,
    intent: Tx,
    inverse: Vec<Op>,
    bindings: BTreeMap<String, String>,
    parent_hash: Option<String>,
    committed_at: i64,
) -> Result<Record, StoreError> {
    let resolved = resolve_ops(&intent.ops, &bindings)?;
    let mut record = Record::new_edit(
        intent.tx_id.clone(),
        seq,
        intent.schema_version,
        parent_hash,
        intent.author.clone(),
        committed_at,
        intent,
        resolved,
        inverse,
        bindings,
    );
    record.set_link(link);
    Ok(record)
}

/// `record`'s link for its [`CommittedTx`]: `None` for an edit.
pub(crate) fn committed_link(record: &Record) -> Result<Option<Box<Link>>, CoreError> {
    if record.kind == RecordKind::Edit {
        return Ok(None);
    }
    record.link().map(|l| Some(Box::new(l)))
}

/// The `record_links` row of a non-`edit` record (none for an edit).
///
/// - `origin`, `origin_hash`, `from_branch`, `merge_id`: as in the record (`merge`,
///   `cherryPick`);
/// - `targets`: a JSON array of what the record acts on: the units of an `undo`, the `undo` of a
///   `redo`, the merge group of a `revert`, the branch of a `branch` or `branchSet`;
/// - `name`: of a `checkpoint`, `branch` or `branchSet`;
/// - `recovers`: the quarantined record a recovery's record stands for (plan 4B Decision 27).
fn insert_link(conn: &Connection, branch: &str, record: &Record) -> Result<(), StoreError> {
    let Some(l) = link_row(branch, record)? else {
        return Ok(());
    };
    conn.execute(
        "INSERT INTO record_links (tx_id, branch, seq, kind, author, origin, origin_hash,
                                   from_branch, merge_id, targets, name, recovers)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            l.tx_id,
            l.branch,
            l.seq,
            l.kind,
            l.author,
            l.origin,
            l.origin_hash,
            l.from_branch,
            l.merge_id,
            l.targets,
            l.name,
            l.recovers,
        ],
    )?;
    Ok(())
}

/// A `record_links` row, without its `ignored` mark ([`insert_link`] has the columns).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkRow {
    pub(crate) tx_id: String,
    pub(crate) branch: String,
    pub(crate) seq: i64,
    pub(crate) kind: String,
    pub(crate) author: String,
    pub(crate) origin: Option<String>,
    pub(crate) origin_hash: Option<String>,
    pub(crate) from_branch: Option<String>,
    pub(crate) merge_id: Option<String>,
    pub(crate) targets: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) recovers: Option<String>,
}

/// The `record_links` row `record` on `branch` gets (none for an edit), as [`insert_link`]
/// writes it.
pub(crate) fn link_row(branch: &str, record: &Record) -> Result<Option<LinkRow>, StoreError> {
    let Some(link) = committed_link(record)? else {
        return Ok(None);
    };
    let link = *link;
    let (mut origin, mut origin_hash, mut from, mut merge_id, mut name) =
        (None, None, None, None, None);
    let targets: Option<Vec<String>> = match link {
        Link::Edit => None,
        Link::Undo { undoes } => Some(undoes),
        Link::Redo { redoes } => Some(vec![redoes]),
        Link::Merge {
            merge_id: m,
            from: f,
            origin: o,
        } => {
            (merge_id, from, origin) = (Some(m), Some(f), Some(o));
            None
        }
        Link::CherryPick {
            origin: o,
            from: f,
            origin_hash: h,
            ..
        } => {
            (origin, from, origin_hash) = (Some(o), Some(f), h);
            None
        }
        Link::Revert { reverts } => Some(vec![reverts]),
        Link::Checkpoint { name: n, .. } => {
            name = Some(n);
            None
        }
        Link::Branch {
            branch_id, name: n, ..
        } => {
            name = Some(n);
            Some(vec![branch_id])
        }
        Link::BranchSet {
            branch_id, name: n, ..
        } => {
            name = n;
            Some(vec![branch_id])
        }
    };
    Ok(Some(LinkRow {
        tx_id: record.tx_id.clone(),
        branch: branch.to_string(),
        seq: to_i64(record.seq)?,
        kind: kind_text(record.kind)?,
        author: record.author.clone(),
        origin,
        origin_hash,
        from_branch: from,
        merge_id,
        targets: targets.map(|t| serde_json::to_string(&t)).transpose()?,
        name,
        recovers: record.recovers.clone(),
    }))
}

/// Appends `record` (committed at `epoch`, not exported) on `branch`. The stored bytes are the record's
/// encoding and `tx_hash` is their hash, so what is hashed is exactly what is stored. Returns
/// that hash.
pub(crate) fn insert_tx(
    conn: &Connection,
    branch: &str,
    record: &Record,
    diff: &Diff,
    epoch: u64,
) -> Result<String, StoreError> {
    let bytes = encode_record(record);
    if bytes.len() > crate::segment::MAX_RECORD_BYTES {
        // Exported, it could not fit a segment that every reader accepts.
        return Err(CoreError::new(
            "TX_TOO_LARGE",
            format!(
                "transaction {} is {} bytes as a record; the limit is {}",
                record.tx_id,
                bytes.len(),
                crate::segment::MAX_RECORD_BYTES
            ),
        )
        .with_details(serde_json::json!({
            "txId": record.tx_id,
            "bytes": bytes.len(),
            "limit": crate::segment::MAX_RECORD_BYTES,
        }))
        .with_fix("split the edit into several smaller transactions")
        .into());
    }
    let tx_hash = hash_bytes(&bytes);
    conn.execute(
        "INSERT INTO txs (seq, branch, tx_id, kind, record_bytes, tx_hash, parent_hash, epoch,
                          exported, diff, schema_version, author, committed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?12, 0, ?8, ?9, ?10, ?11)",
        params![
            to_i64(record.seq)?,
            branch,
            record.tx_id,
            kind_text(record.kind)?,
            bytes,
            tx_hash,
            record.parent_hash,
            serde_json::to_string(diff)?,
            record.schema_version,
            record.author,
            record.committed_at,
            to_i64(epoch)?,
        ],
    )?;
    insert_link(conn, branch, record)?;
    Ok(tx_hash)
}

/// Appends a record taken from the project folder: its exact `bytes` (whose hash is
/// `tx_hash`), already exported, at `epoch`. `schema_version` is the version the bytes were
/// written with (`record` is upcast). `diff` `None`: not computed yet (a rebuild seeded from a
/// snapshot, [`crate::diffs`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_imported(
    conn: &Connection,
    branch: &str,
    record: &Record,
    bytes: &[u8],
    tx_hash: &str,
    schema_version: u32,
    diff: Option<&Diff>,
    epoch: u64,
) -> Result<(), StoreError> {
    let diff = match diff {
        Some(d) => serde_json::to_string(d)?,
        None => crate::diffs::NOT_STORED.to_string(),
    };
    conn.execute(
        "INSERT INTO txs (seq, branch, tx_id, kind, record_bytes, tx_hash, parent_hash, epoch,
                          exported, diff, schema_version, author, committed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11, ?12)",
        params![
            to_i64(record.seq)?,
            branch,
            record.tx_id,
            kind_text(record.kind)?,
            bytes,
            tx_hash,
            record.parent_hash,
            to_i64(epoch)?,
            diff,
            schema_version,
            record.author,
            record.committed_at,
        ],
    )?;
    insert_link(conn, branch, record)?;
    Ok(())
}

/// A `txs` row as stored: `(seq, tx_id, record_bytes)`.
pub(crate) type RecordRow = (u64, String, Vec<u8>);

/// The raw rows of `branch` with `seq > after`, in order.
pub(crate) fn record_rows(
    conn: &Connection,
    branch: &str,
    after: u64,
) -> Result<Vec<RecordRow>, StoreError> {
    record_rows_in(conn, branch, after, None)
}

/// The raw rows of `branch` with `after < seq <= through` (`None`: no upper bound), in order.
pub(crate) fn record_rows_in(
    conn: &Connection,
    branch: &str,
    after: u64,
    through: Option<u64>,
) -> Result<Vec<RecordRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, tx_id, record_bytes FROM txs WHERE branch = ?1 AND seq > ?2 AND seq <= ?3
         ORDER BY seq",
    )?;
    let through = through.map_or(Ok(i64::MAX), to_i64)?;
    let rows = stmt.query_map(params![branch, to_i64(after)?, through], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    rows.map(|r| {
        let (seq, tx_id, bytes) = r?;
        Ok((to_u64(seq)?, tx_id, bytes))
    })
    .collect()
}

/// The `kind` column: the record's serialised, camelCase kind (`edit`, `cherryPick`).
pub(crate) fn kind_text(kind: RecordKind) -> Result<String, StoreError> {
    Ok(fin_core::kind_name(kind))
}

const TX_COLUMNS: &str = "seq, record_bytes, diff";

fn committed_from_row(row: &Row<'_>) -> rusqlite::Result<(i64, Vec<u8>, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

/// Bytes to a [`Record`]: not JSON, or not a record, is `INVALID_FORMAT` with
/// `details {kind: "record", reason}`, plus `seq` when the row is known.
pub(crate) fn parse_record(seq: Option<u64>, bytes: &[u8]) -> Result<Record, CoreError> {
    parse_record_with_diff(seq, bytes, None)
        .map(|(r, _)| r)
        .map_err(CoreError::from)
}

/// [`parse_record`], and the row's stored diff through [`stored_diff`] when `diff` is given:
/// the record's bytes are parsed once for both.
pub(crate) fn parse_record_with_diff(
    seq: Option<u64>,
    bytes: &[u8],
    diff: Option<&str>,
) -> Result<(Record, Option<Diff>), StoreError> {
    let with_seq = |mut e: CoreError| {
        if let (Some(seq), Value::Object(map)) = (seq, &mut e.details) {
            map.insert("seq".into(), serde_json::json!(seq));
        }
        e
    };
    let value: Value = serde_json::from_slice(bytes).map_err(|e| {
        with_seq(
            CoreError::new(
                "INVALID_FORMAT",
                format!("a stored record is not JSON: {e}"),
            )
            .with_details(serde_json::json!({ "kind": "record", "reason": e.to_string() })),
        )
    })?;
    let diff = diff
        .map(|d| {
            stored_diff(&value, d).map_err(|e| match e {
                StoreError::Core(e) => with_seq(e).into(),
                e => e,
            })
        })
        .transpose()?;
    Ok((upcast_record(value).map_err(with_seq)?, diff))
}

/// A row's `txs.diff` in the current shape, for its record's raw JSON `record` (plan 4C
/// Decision 5). Every reader of `txs.diff` goes through here: a diff stored by 4B or earlier
/// (v1-shaped, never rewritten) is upcast against the record's raw v1 `intent.ops`.
pub(crate) fn stored_diff(record: &Value, diff: &str) -> Result<Diff, StoreError> {
    let diff: Value = serde_json::from_str(diff)?;
    let v1_ops: &[Value] = if record["schemaVersion"] == 1 {
        record["intent"]["ops"]
            .as_array()
            .map_or(&[], Vec::as_slice)
    } else {
        &[]
    };
    Ok(upcast_diff(diff, v1_ops)?)
}

fn decode((seq, bytes, diff): (i64, Vec<u8>, String)) -> Result<CommittedTx, StoreError> {
    let seq = to_u64(seq)?;
    let (record, diff) = parse_record_with_diff(Some(seq), &bytes, Some(&diff))?;
    Ok(CommittedTx {
        rev: seq,
        link: committed_link(&record)?,
        tx: record.intent,
        diff: diff.unwrap_or_default(),
        bindings: record.bindings,
    })
}

pub(crate) fn find_tx(conn: &Connection, tx_id: &str) -> Result<Option<CommittedTx>, StoreError> {
    let sql = format!("SELECT {TX_COLUMNS}, branch FROM txs WHERE tx_id = ?1");
    let read = || -> Result<Option<_>, StoreError> {
        Ok(conn
            .query_row(&sql, params![tx_id], |r| {
                Ok((committed_from_row(r)?, r.get::<_, String>(3)?))
            })
            .optional()?)
    };
    let mut row = read()?;
    if let Some(((_, _, diff), branch)) = &row
        && diff == crate::diffs::NOT_STORED
    {
        crate::diffs::fill(conn, branch)?;
        row = read()?;
    }
    row.map(|(r, _)| decode(r)).transpose()
}

/// Committed txs of `branch`'s own rows with `after < rev <= through` (`None`: no upper
/// bound), in rev order, as stored: the caller fills pending diffs first
/// ([`crate::branch::lineage::records_since`], which reads a branch's whole lineage).
pub(crate) fn txs_in(
    conn: &Connection,
    branch: &str,
    after: u64,
    through: Option<u64>,
) -> Result<Vec<CommittedTx>, StoreError> {
    let sql = format!(
        "SELECT {TX_COLUMNS} FROM txs WHERE branch = ?1 AND seq > ?2 AND seq <= ?3 ORDER BY seq"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let through = through.map_or(Ok(i64::MAX), to_i64)?;
    let rows = stmt.query_map(params![branch, to_i64(after)?, through], committed_from_row)?;
    rows.map(|r| decode(r?)).collect()
}

/// Applies the stored record `bytes` to `project`: upcasts it and re-applies its intent with the
/// recorded bindings fixing every id ([`apply_committed`]: structural checks only, no policy and
/// no comparison with the stored diff), so history is never re-judged.
///
/// Structural damage is `REPLAY_FAILED` with `details {txId, cause}`. Bytes that are not JSON, or
/// not a record (or newer than this Finisher), are the error of [`parse_record`].
pub fn replay_record(project: &Project, bytes: &[u8]) -> Result<Applied, StoreError> {
    let record = parse_record(None, bytes)?;
    Ok(apply_committed(project, &record.intent, &record.bindings)?)
}

/// [`replay_record`] for the row `(seq, tx_id)` of `branch`. The record must be the row's own:
/// a different `seq`, `txId` or branch is `REPLAY_FAILED` with `details {seq, txId, recordSeq,
/// recordTxId, branch}` (damage, like any record that does not belong in the history). Failures
/// carry `details.seq`.
pub(crate) fn replay_row(
    project: &Project,
    branch: &str,
    (seq, tx_id, bytes): &RecordRow,
) -> Result<Applied, StoreError> {
    let record = parse_record(Some(*seq), bytes)?;
    if record.seq != *seq || record.tx_id != *tx_id || record.intent.branch != branch {
        return Err(CoreError::new(
            "REPLAY_FAILED",
            format!("the record stored at seq {seq} (tx {tx_id}) is another record"),
        )
        .with_details(serde_json::json!({
            "seq": seq, "txId": tx_id, "branch": branch,
            "recordSeq": record.seq, "recordTxId": record.tx_id,
            "recordBranch": record.intent.branch,
        }))
        .with_fix("the stored history is damaged; restore the project from a backup")
        .into());
    }
    apply_committed(project, &record.intent, &record.bindings).map_err(|mut e| {
        if let serde_json::Value::Object(map) = &mut e.details {
            map.insert("seq".into(), serde_json::json!(seq));
        }
        e.into()
    })
}
