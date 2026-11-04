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
use std::time::{SystemTime, UNIX_EPOCH};

use fin_core::{
    Applied, CommittedTx, CoreError, IdAlloc, IdKind, MemIds, Op, Project, Record, RecordKind, Tx,
    apply_committed, encode_record, resolve_ops, upcast_record, upcast_snapshot,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

use crate::StoreError;
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
    origin TEXT NOT NULL,
    reason TEXT NOT NULL,
    record_bytes BLOB NOT NULL,
    added_at INTEGER NOT NULL
);
";

/// Opens (`create`: creates) the database and sets WAL, `synchronous=FULL` and foreign keys.
pub(crate) fn connect(path: &Path, create: bool) -> Result<Connection, StoreError> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let conn = Connection::open_with_flags(path, flags)?;
    let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(CoreError::new(
            "DB_ERROR",
            format!("could not enable WAL (journal_mode is {mode})"),
        )
        .into());
    }
    conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
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
    for (key, value) in [
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

/// Appends `tx` at `seq` as a hashed record chained to the branch's previous record (epoch 0,
/// not exported).
pub(crate) fn insert_tx(
    conn: &Connection,
    seq: u64,
    tx: &Tx,
    applied: &Applied,
) -> Result<(), StoreError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let parent: Option<String> = conn
        .query_row(
            "SELECT tx_hash FROM txs WHERE branch = ?1 ORDER BY seq DESC LIMIT 1",
            params![tx.branch],
            |r| r.get(0),
        )
        .optional()?;
    let record = new_record(
        seq,
        tx.clone(),
        applied.inverse.clone(),
        applied.bindings.clone(),
        parent,
        i64::try_from(now).unwrap_or(i64::MAX),
    )?;
    let bytes = encode_record(&record);
    conn.execute(
        "INSERT INTO txs (seq, branch, tx_id, kind, record_bytes, tx_hash, parent_hash, epoch,
                          exported, diff, schema_version, author, committed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, ?8, ?9, ?10, ?11)",
        params![
            to_i64(seq)?,
            tx.branch,
            record.tx_id,
            kind_text(record.kind)?,
            bytes,
            hash_bytes(&bytes),
            record.parent_hash,
            serde_json::to_string(&applied.diff)?,
            record.schema_version,
            record.author,
            record.committed_at,
        ],
    )?;
    Ok(())
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

/// A stored record that is not JSON, or not a record, is `INVALID_FORMAT` with
/// `details {kind: "record", seq, reason}`.
fn decode((seq, bytes, diff): (i64, Vec<u8>, String)) -> Result<CommittedTx, StoreError> {
    let with_seq = |mut e: CoreError| {
        if let serde_json::Value::Object(map) = &mut e.details {
            map.insert("seq".into(), serde_json::json!(seq));
        }
        e
    };
    let value = serde_json::from_slice(&bytes).map_err(|e| {
        with_seq(
            CoreError::new(
                "INVALID_FORMAT",
                format!("the record at seq {seq} is not JSON: {e}"),
            )
            .with_details(serde_json::json!({ "kind": "record", "reason": e.to_string() })),
        )
    })?;
    let record = upcast_record(value).map_err(with_seq)?;
    Ok(CommittedTx {
        rev: to_u64(seq)?,
        tx: record.intent,
        diff: serde_json::from_str(&diff)?,
        bindings: record.bindings,
    })
}

pub(crate) fn find_tx(conn: &Connection, tx_id: &str) -> Result<Option<CommittedTx>, StoreError> {
    let sql = format!("SELECT {TX_COLUMNS} FROM txs WHERE tx_id = ?1");
    conn.query_row(&sql, params![tx_id], committed_from_row)
        .optional()?
        .map(decode)
        .transpose()
}

/// Committed txs on `branch` with `rev > after`, in rev order.
pub(crate) fn txs_since(
    conn: &Connection,
    branch: &str,
    after: u64,
) -> Result<Vec<CommittedTx>, StoreError> {
    let sql = format!("SELECT {TX_COLUMNS} FROM txs WHERE branch = ?1 AND seq > ?2 ORDER BY seq");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![branch, to_i64(after)?], committed_from_row)?;
    rows.map(|r| decode(r?)).collect()
}

const KINDS: [IdKind; 4] = [IdKind::Asset, IdKind::Track, IdKind::Clip, IdKind::Timeline];

fn kind_name(kind: IdKind) -> &'static str {
    match kind {
        IdKind::Asset => "asset",
        IdKind::Track => "track",
        IdKind::Clip => "clip",
        IdKind::Timeline => "timeline",
    }
}

fn slot(kind: IdKind) -> usize {
    KINDS.iter().position(|k| *k == kind).unwrap_or(0)
}

/// Numeric suffix of `id` when it is exactly `prefix` followed by digits.
fn suffix(id: &str, prefix: &str) -> Option<u64> {
    let digits = id.strip_prefix(prefix)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The `ids` table loaded into memory for one SQLite transaction and saved back in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Counters {
    next: [u64; 4],
}

impl Counters {
    /// Counters past every id in `project`, by `fin-core`'s own rule (`MemIds::after`).
    pub(crate) fn seed(project: &Project) -> Counters {
        let mut mem = MemIds::after(project);
        let next = KINDS.map(|k| suffix(&mem.next(k), k.prefix()).unwrap_or(1));
        Counters { next }
    }

    pub(crate) fn load(conn: &Connection) -> Result<Counters, StoreError> {
        let mut next = [1; 4];
        for kind in KINDS {
            let n: i64 = conn.query_row(
                "SELECT next FROM ids WHERE kind = ?1",
                params![kind_name(kind)],
                |r| r.get(0),
            )?;
            next[slot(kind)] = to_u64(n)?;
        }
        Ok(Counters { next })
    }

    pub(crate) fn save(&self, conn: &Connection) -> Result<(), StoreError> {
        for kind in KINDS {
            conn.execute(
                "INSERT INTO ids (kind, next) VALUES (?1, ?2) ON CONFLICT(kind) DO UPDATE SET next = excluded.next",
                params![kind_name(kind), to_i64(self.next[slot(kind)])?],
            )?;
        }
        Ok(())
    }
}

impl IdAlloc for Counters {
    fn next(&mut self, kind: IdKind) -> String {
        let n = &mut self.next[slot(kind)];
        let id = format!("{}{}", kind.prefix(), n);
        *n += 1;
        id
    }
}

/// `project` with the committed tx `c` applied again ([`fin_core::apply_committed`]: the
/// recorded bindings fix every id, structural checks only), producing the same ids, bindings and
/// diff as when it was committed. Used to rebuild heads from snapshots and by replay checks.
pub fn replay(project: &Project, c: &CommittedTx) -> Result<Project, StoreError> {
    let applied =
        apply_committed(project, &c.tx, &c.bindings).map_err(|e| replay_error(c, Some(e)))?;
    if applied.diff != c.diff {
        return Err(replay_error(c, None));
    }
    Ok(applied.project)
}

fn replay_error(c: &CommittedTx, cause: Option<CoreError>) -> StoreError {
    let mut details = BTreeMap::new();
    details.insert("rev", serde_json::json!(c.rev));
    details.insert("txId", serde_json::json!(c.tx.tx_id));
    if let Some(e) = cause {
        details.insert("cause", serde_json::json!(e));
    }
    CoreError::new(
        "DB_ERROR",
        format!(
            "committed tx {} (rev {}) does not replay",
            c.tx.tx_id, c.rev
        ),
    )
    .with_details(serde_json::json!(details))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_core::{Rational, new_project};

    #[test]
    fn seed_starts_past_the_new_project_ids() {
        let p = new_project("p".into(), "n".into(), Rational::from_int(30)).unwrap();
        let mut c = Counters::seed(&p);
        assert_eq!(
            KINDS.map(|k| c.next(k)),
            [
                "a1".to_string(),
                "t3".to_string(),
                "c1".to_string(),
                "tl2".to_string()
            ]
        );
    }
}
