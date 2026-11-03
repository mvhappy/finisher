//! SQLite layout and row access for `project.fdb`.
//!
//! One database per project. `txs` is the op log per branch; `snapshots` holds the full project
//! at rev 0 and every [`SNAPSHOT_EVERY`] revs so opening replays at most that many txs; `ids` is
//! the project-global id counter table, updated in the same SQLite transaction as the `txs`
//! insert so a failed commit never consumes ids.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use fin_core::{
    Applied, CommittedTx, CoreError, IdAlloc, IdKind, MemIds, Op, Project, Tx, apply_tx,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

use crate::StoreError;

/// A snapshot is written at rev 0 and whenever `rev % SNAPSHOT_EVERY == 0`.
pub const SNAPSHOT_EVERY: u64 = 200;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS ids (kind TEXT PRIMARY KEY, next INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS txs (
    rev INTEGER NOT NULL,
    branch TEXT NOT NULL,
    tx_id TEXT NOT NULL UNIQUE,
    json TEXT NOT NULL,
    diff TEXT NOT NULL,
    inverse TEXT NOT NULL,
    bindings TEXT NOT NULL,
    committed_at INTEGER NOT NULL,
    PRIMARY KEY (branch, rev)
);
CREATE TABLE IF NOT EXISTS snapshots (
    branch TEXT NOT NULL,
    rev INTEGER NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (branch, rev)
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

/// Creates the tables and stores the rev-0 state of `project` on `branch`.
pub(crate) fn init(
    conn: &mut Connection,
    project: &Project,
    branch: &str,
) -> Result<(), StoreError> {
    let txn = conn.transaction()?;
    txn.execute_batch(SCHEMA)?;
    txn.execute(
        "INSERT INTO meta (key, value) VALUES ('projectId', ?1)",
        params![project.id],
    )?;
    txn.execute(
        "INSERT INTO meta (key, value) VALUES ('schemaVersion', ?1)",
        params![project.schema_version.to_string()],
    )?;
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

fn to_i64(n: u64) -> Result<i64, StoreError> {
    i64::try_from(n).map_err(|_| {
        CoreError::new("DB_ERROR", format!("value {n} does not fit the database")).into()
    })
}

fn to_u64(n: i64) -> Result<u64, StoreError> {
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
        "INSERT INTO snapshots (branch, rev, json) VALUES (?1, ?2, ?3)",
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

/// The newest snapshot of `branch`.
pub(crate) fn latest_snapshot(
    conn: &Connection,
    branch: &str,
) -> Result<Option<(u64, Project)>, StoreError> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT rev, json FROM snapshots WHERE branch = ?1 ORDER BY rev DESC LIMIT 1",
            params![branch],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(rev, json)| Ok((to_u64(rev)?, serde_json::from_str(&json)?)))
        .transpose()
}

pub(crate) fn insert_tx(
    conn: &Connection,
    rev: u64,
    tx: &Tx,
    applied: &Applied,
) -> Result<(), StoreError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    conn.execute(
        "INSERT INTO txs (rev, branch, tx_id, json, diff, inverse, bindings, committed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            to_i64(rev)?,
            tx.branch,
            tx.tx_id,
            serde_json::to_string(tx)?,
            serde_json::to_string(&applied.diff)?,
            serde_json::to_string(&applied.inverse)?,
            serde_json::to_string(&applied.bindings)?,
            i64::try_from(now).unwrap_or(i64::MAX),
        ],
    )?;
    Ok(())
}

const TX_COLUMNS: &str = "rev, json, diff, bindings";

fn committed_from_row(row: &Row<'_>) -> rusqlite::Result<(i64, String, String, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn decode(
    (rev, json, diff, bindings): (i64, String, String, String),
) -> Result<CommittedTx, StoreError> {
    Ok(CommittedTx {
        rev: to_u64(rev)?,
        tx: serde_json::from_str(&json)?,
        diff: serde_json::from_str(&diff)?,
        bindings: serde_json::from_str(&bindings)?,
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
    let sql = format!("SELECT {TX_COLUMNS} FROM txs WHERE branch = ?1 AND rev > ?2 ORDER BY rev");
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

/// Hands out a committed tx's bound ids again, in op order, so replay reproduces it exactly.
struct ReplayIds {
    queue: VecDeque<String>,
    mismatch: bool,
}

impl IdAlloc for ReplayIds {
    fn next(&mut self, kind: IdKind) -> String {
        match self.queue.pop_front() {
            Some(id) if suffix(&id, kind.prefix()).is_some() => id,
            _ => {
                self.mismatch = true;
                String::new()
            }
        }
    }
}

/// `project` with the committed tx `c` applied again, producing the same ids, bindings and diff
/// as when it was committed. Used to rebuild heads from snapshots and by replay checks.
pub fn replay(project: &Project, c: &CommittedTx) -> Result<Project, StoreError> {
    let queue =
        c.tx.ops
            .iter()
            .filter_map(|op| match op {
                Op::AssetAdd { id, .. } | Op::ClipInsert { id, .. } => c.bindings.get(id).cloned(),
                _ => None,
            })
            .collect();
    let mut ids = ReplayIds {
        queue,
        mismatch: false,
    };
    let applied = apply_tx(project, &c.tx, &mut ids).map_err(|e| replay_error(c, Some(e)))?;
    if ids.mismatch
        || !ids.queue.is_empty()
        || applied.bindings != c.bindings
        || applied.diff != c.diff
    {
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
