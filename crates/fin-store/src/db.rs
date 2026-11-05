//! SQLite layout and row access for `project.fdb`.
//!
//! One database per project, at schema [`DB_VERSION`] (older ones are migrated on open,
//! [`crate::migrations`]). `txs` is the op log per branch: each row holds the commit record's
//! exact bytes (`record_bytes`), their hash and the previous record's hash; `snapshots` holds the
//! full project at rev 0 and every [`SNAPSHOT_EVERY`] revs so opening replays at most that many
//! txs; `ids` is the project-global id counter table, updated in the same SQLite transaction as
//! the `txs` insert so a failed commit never consumes ids. `segments`, `lease_state` and
//! `quarantine` belong to the project-folder export (sub-project 4A).

use std::collections::BTreeMap;
use std::path::Path;

use fin_core::{
    Applied, CommittedTx, CoreError, Diff, Op, Project, Record, RecordKind, Tx, apply_committed,
    encode_record, resolve_ops, upcast_record, upcast_snapshot,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

use crate::StoreError;
pub(crate) use crate::counters::Counters;
use crate::migrations::{APPLICATION_ID, DB_VERSION, MIN_READER_VERSION};

/// A snapshot is written at rev 0 and whenever `rev % SNAPSHOT_EVERY == 0`.
pub const SNAPSHOT_EVERY: u64 = 200;

/// The current schema, created directly for a new project (no migrations). Migrations bring an
/// older database to the same tables (`tests/migrations.rs` compares the two).
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS ids (kind TEXT PRIMARY KEY, next INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS txs (
    seq INTEGER NOT NULL,
    branch TEXT NOT NULL,
    tx_id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    record_bytes BLOB NOT NULL,
    tx_hash TEXT NOT NULL,
    parent_hash TEXT,
    epoch INTEGER NOT NULL,
    exported INTEGER NOT NULL,
    diff TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    author TEXT NOT NULL,
    committed_at INTEGER NOT NULL,
    PRIMARY KEY (branch, seq)
);
CREATE INDEX IF NOT EXISTS txs_unexported ON txs (branch, seq) WHERE exported = 0;
CREATE TABLE IF NOT EXISTS snapshots (
    branch TEXT NOT NULL,
    seq INTEGER NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (branch, seq)
);
CREATE TABLE IF NOT EXISTS segments (
    branch TEXT NOT NULL,
    hash TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    first_seq INTEGER NOT NULL,
    last_seq INTEGER NOT NULL,
    pack INTEGER NOT NULL,
    replaces TEXT NOT NULL,
    state TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS lease_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS quarantine (
    id INTEGER PRIMARY KEY,
    tx_hash TEXT NOT NULL UNIQUE,
    branch TEXT NOT NULL,
    origin TEXT NOT NULL,
    reason TEXT NOT NULL,
    detail TEXT NOT NULL,
    record_bytes BLOB NOT NULL,
    added_at INTEGER NOT NULL,
    seen_at INTEGER NOT NULL,
    current INTEGER NOT NULL
);
";

/// Opens (`create`: creates) the database and sets WAL, `synchronous=FULL` and foreign keys.
pub(crate) fn connect(path: &Path, create: bool) -> Result<Connection, StoreError> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let conn = Connection::open_with_flags(path, flags)?;
    // First: switching to WAL takes an exclusive lock, and two opens may race to do it.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // The busy handler does not cover a journal-mode switch: retry it briefly by hand.
    let mut tries = 0;
    let mode: String = loop {
        match conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)) {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy && tries < 100 =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            other => break other?,
        }
    };
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(CoreError::new(
            "DB_ERROR",
            format!("could not enable WAL (journal_mode is {mode})"),
        )
        .into());
    }
    conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")?;
    Ok(conn)
}

/// Creates the tables at [`DB_VERSION`] (header `application_id` and `user_version`, the
/// version `meta` keys) and stores the rev-0 state of `project` on `branch`.
pub(crate) fn init(
    conn: &mut Connection,
    project: &Project,
    branch: &str,
) -> Result<(), StoreError> {
    let txn = conn.transaction()?;
    txn.execute_batch(SCHEMA)?;
    txn.pragma_update(None, "application_id", APPLICATION_ID)?;
    txn.pragma_update(None, "user_version", DB_VERSION)?;
    let version = DB_VERSION.to_string();
    let min_reader = MIN_READER_VERSION.to_string();
    let by = written_by();
    let incarnation = crate::history::new_incarnation();
    let genesis = crate::project_file::genesis_hash(&crate::project_file::genesis_of(project));
    for (key, value) in [
        ("incarnation", incarnation.as_str()),
        ("genesisHash", genesis.as_str()),
        ("projectId", project.id.as_str()),
        ("schemaVersion", &project.schema_version.to_string()),
        ("formatVersion", &version),
        ("minReaderVersion", &min_reader),
        ("createdBy", &by),
        ("lastOpenedBy", &by),
    ] {
        set_meta(&txn, key, value)?;
    }
    Counters::seed(project).save(&txn)?;
    insert_snapshot(&txn, branch, 0, project)?;
    txn.commit()?;
    Ok(())
}

pub(crate) fn meta(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()?)
}

pub(crate) fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value WHERE value != excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// `finisher/<version>`: what `meta.createdBy` and `meta.lastOpenedBy` record. Never a host or
/// user name.
pub(crate) fn written_by() -> String {
    format!("finisher/{}", env!("CARGO_PKG_VERSION"))
}

/// `blake3:<64 lowercase hex>` of `b`.
pub(crate) fn hash_bytes(b: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(b).to_hex())
}

/// The record a commit of `intent` at `seq` stores (spec §3.3). Migration 0001 builds the
/// records of sub-project 2 history with this too, so both are the same for the same tx.
pub(crate) fn new_record(
    seq: u64,
    intent: Tx,
    inverse: Vec<Op>,
    bindings: BTreeMap<String, String>,
    parent_hash: Option<String>,
    committed_at: i64,
) -> Result<Record, StoreError> {
    Ok(Record {
        tx_id: intent.tx_id.clone(),
        seq,
        kind: RecordKind::Edit,
        schema_version: intent.schema_version,
        parent_hash,
        author: intent.author.clone(),
        committed_at,
        resolved: resolve_ops(&intent.ops, &bindings)?,
        intent,
        inverse,
        bindings,
    })
}

pub(crate) fn to_i64(n: u64) -> Result<i64, StoreError> {
    i64::try_from(n).map_err(|_| {
        CoreError::new("DB_ERROR", format!("value {n} does not fit the database")).into()
    })
}

pub(crate) fn to_u64(n: i64) -> Result<u64, StoreError> {
    u64::try_from(n).map_err(|_| {
        CoreError::new("DB_ERROR", format!("negative value {n} in the database")).into()
    })
}

pub(crate) fn insert_snapshot(
    conn: &Connection,
    branch: &str,
    rev: u64,
    project: &Project,
) -> Result<(), StoreError> {
    let json = serde_json::to_string(project)?;
    conn.execute(
        "INSERT INTO snapshots (branch, seq, json) VALUES (?1, ?2, ?3)",
        params![branch, to_i64(rev)?, json],
    )?;
    Ok(())
}

/// Every branch that has a snapshot (each branch gets one at its creation).
pub(crate) fn branches(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT DISTINCT branch FROM snapshots ORDER BY branch")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The newest rev of `branch` (its latest tx, else its latest snapshot); `None` when the branch
/// does not exist.
pub(crate) fn latest_rev(conn: &Connection, branch: &str) -> Result<Option<u64>, StoreError> {
    let rev: Option<i64> = conn.query_row(
        "SELECT MAX(seq) FROM (SELECT seq FROM txs WHERE branch = ?1
                               UNION ALL SELECT seq FROM snapshots WHERE branch = ?1)",
        params![branch],
        |r| r.get(0),
    )?;
    rev.map(to_u64).transpose()
}

/// The newest snapshot of `branch`.
pub(crate) fn latest_snapshot(
    conn: &Connection,
    branch: &str,
) -> Result<Option<(u64, Project)>, StoreError> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, json FROM snapshots WHERE branch = ?1 ORDER BY seq DESC LIMIT 1",
            params![branch],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(rev, json)| {
        let value: serde_json::Value = serde_json::from_str(&json)?;
        Ok((to_u64(rev)?, upcast_snapshot(value)?))
    })
    .transpose()
}

/// The highest commit (or import) epoch of any record, 0 for none.
pub(crate) fn max_epoch(conn: &Connection) -> Result<u64, StoreError> {
    let e: Option<i64> = conn.query_row("SELECT MAX(epoch) FROM txs", [], |r| r.get(0))?;
    e.map_or(Ok(0), to_u64)
}

/// The snapshot of `branch` at exactly `seq`, if there is one.
pub(crate) fn snapshot_at(
    conn: &Connection,
    branch: &str,
    seq: u64,
) -> Result<Option<Project>, StoreError> {
    let json: Option<String> = conn
        .query_row(
            "SELECT json FROM snapshots WHERE branch = ?1 AND seq = ?2",
            params![branch, to_i64(seq)?],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|j| Ok(upcast_snapshot(serde_json::from_str(&j)?)?))
        .transpose()
}

/// Hash of the newest record on `branch`: the `parent_hash` of the next one (`None` for the
/// first).
pub(crate) fn tip_hash(conn: &Connection, branch: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT tx_hash FROM txs WHERE branch = ?1 ORDER BY seq DESC LIMIT 1",
            params![branch],
            |r| r.get(0),
        )
        .optional()?)
}

/// `txHash` of the row at `seq` of `branch` (`None` when there is none, e.g. rev 0).
pub(crate) fn hash_at(
    conn: &Connection,
    branch: &str,
    seq: u64,
) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT tx_hash FROM txs WHERE branch = ?1 AND seq = ?2",
            params![branch, to_i64(seq)?],
            |r| r.get(0),
        )
        .optional()?)
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
    let mut stmt = conn.prepare(
        "SELECT seq, tx_id, record_bytes FROM txs WHERE branch = ?1 AND seq > ?2 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![branch, to_i64(after)?], |r| {
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

/// The `kind` column: the record's serialised kind (`"edit"` → `edit`).
fn kind_text(kind: RecordKind) -> Result<String, StoreError> {
    match serde_json::to_value(kind)? {
        serde_json::Value::String(s) => Ok(s),
        other => {
            Err(CoreError::new("DB_ERROR", format!("record kind {other} is not a string")).into())
        }
    }
}

const TX_COLUMNS: &str = "seq, record_bytes, diff";

fn committed_from_row(row: &Row<'_>) -> rusqlite::Result<(i64, Vec<u8>, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

/// Bytes to a [`Record`]: not JSON, or not a record, is `INVALID_FORMAT` with
/// `details {kind: "record", reason}`, plus `seq` when the row is known.
pub(crate) fn parse_record(seq: Option<u64>, bytes: &[u8]) -> Result<Record, CoreError> {
    let with_seq = |mut e: CoreError| {
        if let (Some(seq), serde_json::Value::Object(map)) = (seq, &mut e.details) {
            map.insert("seq".into(), serde_json::json!(seq));
        }
        e
    };
    let value = serde_json::from_slice(bytes).map_err(|e| {
        with_seq(
            CoreError::new(
                "INVALID_FORMAT",
                format!("a stored record is not JSON: {e}"),
            )
            .with_details(serde_json::json!({ "kind": "record", "reason": e.to_string() })),
        )
    })?;
    upcast_record(value).map_err(with_seq)
}

fn decode((seq, bytes, diff): (i64, Vec<u8>, String)) -> Result<CommittedTx, StoreError> {
    let seq = to_u64(seq)?;
    let record = parse_record(Some(seq), &bytes)?;
    Ok(CommittedTx {
        rev: seq,
        tx: record.intent,
        diff: serde_json::from_str(&diff)?,
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

/// Committed txs on `branch` with `rev > after`, in rev order.
pub(crate) fn txs_since(
    conn: &Connection,
    branch: &str,
    after: u64,
) -> Result<Vec<CommittedTx>, StoreError> {
    if crate::diffs::pending(conn, branch)?.is_some_and(|through| through > after) {
        crate::diffs::fill(conn, branch)?;
    }
    let sql = format!("SELECT {TX_COLUMNS} FROM txs WHERE branch = ?1 AND seq > ?2 ORDER BY seq");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![branch, to_i64(after)?], committed_from_row)?;
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
