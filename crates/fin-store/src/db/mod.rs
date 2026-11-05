//! SQLite layout and row access for `project.fdb`.
//!
//! One database per project, at schema [`DB_VERSION`] (older ones are migrated on open,
//! [`crate::migrations`]). `txs` is the op log per branch: each row holds the commit record's
//! exact bytes (`record_bytes`), their hash and the previous record's hash; `snapshots` holds the
//! full project at rev 0 and every [`SNAPSHOT_EVERY`] revs so opening replays at most that many
//! txs; `ids` is the project-global id counter table, updated in the same SQLite transaction as
//! the `txs` insert so a failed commit never consumes ids. `segments`, `lease_state` and
//! `quarantine` belong to the project-folder export (sub-project 4A). `branches` and
//! `record_links` are 4B's derived history tables, rebuildable from `txs`.

use std::path::Path;

use fin_core::{CoreError, Project, upcast_snapshot};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::StoreError;
pub(crate) use crate::counters::Counters;
use crate::migrations::{APPLICATION_ID, DB_VERSION, MIN_READER_VERSION};

mod records;
pub use records::replay_record;
pub(crate) use records::*;

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
CREATE TABLE IF NOT EXISTS branches (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, parent TEXT, fork_seq INTEGER NOT NULL,
    fork_hash TEXT, created_seq INTEGER, created_by TEXT NOT NULL, created_at INTEGER NOT NULL,
    archived INTEGER NOT NULL, merged INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS record_links (
    tx_id TEXT PRIMARY KEY, branch TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL,
    author TEXT NOT NULL, origin TEXT, origin_hash TEXT, from_branch TEXT, merge_id TEXT,
    targets TEXT, name TEXT, UNIQUE (branch, seq)
);
CREATE INDEX IF NOT EXISTS record_links_origin ON record_links (origin) WHERE origin IS NOT NULL;
CREATE INDEX IF NOT EXISTS record_links_origin_hash ON record_links (origin_hash) WHERE origin_hash IS NOT NULL;
CREATE INDEX IF NOT EXISTS record_links_merge ON record_links (merge_id) WHERE merge_id IS NOT NULL;
";

/// The `main` row of `branches`, as migration 0002 writes it.
const MAIN_BRANCH_ROW: &str =
    "INSERT INTO branches VALUES ('main','main',NULL,0,NULL,NULL,'',0,0,0)";

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
/// version `meta` keys, the `main` row of `branches`) and stores the rev-0 state of `project`
/// on `branch`.
pub(crate) fn init(
    conn: &mut Connection,
    project: &Project,
    branch: &str,
) -> Result<(), StoreError> {
    let txn = conn.transaction()?;
    txn.execute_batch(SCHEMA)?;
    txn.execute(MAIN_BRANCH_ROW, [])?;
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
