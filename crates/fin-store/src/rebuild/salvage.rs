//! What a rebuild keeps from the databases it replaces (`super`, step 4): the old database,
//! damaged copies kept by earlier rebuilds that it has not salvaged, and unfinished builds.
//!
//! - Every `txs` row whose record the new database does not hold goes to `quarantine`
//!   (origin `salvage`, bytes kept): `SALVAGED` when it was never exported, and
//!   `MISSING_FROM_FOLDER` when it was exported but the folder no longer yields it (a
//!   damaged or deleted segment file). Both stay current (nothing else holds them).
//! - Its quarantine rows, with their reason; its id counters (never lowered); its
//!   `projectDir`, server ids and `salvagedFrom`.
//!
//! The old file may be damaged anywhere. Tables are read by rowid ranges: after a read error
//! the reader probes further rowids and resumes past the bad page (a binary search finds the
//! first readable rowid), so one bad page loses only its own rows; a row that is read but
//! whose columns do not decode loses only itself. Every error is reported
//! ([`Outcome::errors`]); `complete` is false whenever anything could not be read.
//! Sources are opened read-only, so a kept copy's WAL is never checkpointed into it.

use std::path::Path;

use fin_core::Project;
use rusqlite::{Connection, OpenFlags, Row, params};

use super::check::open_read_only;
use crate::StoreError;
use crate::db::{self, hash_bytes};

/// What one source gave.
#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub(super) added: u32,
    /// Exported rows of the source that the new database lacks.
    pub(super) lost_exported: u32,
    /// The highest exported seq of `main` in the source.
    pub(super) exported_through: u64,
    pub(super) errors: Vec<String>,
    /// `salvagedFrom` of the source: damaged copies already salvaged into it.
    pub(super) salvaged_from: Vec<String>,
    /// The source's own record of incomplete salvages.
    pub(super) incomplete: Vec<Incomplete>,
}

/// A kept copy whose salvage could not read everything (`meta` `salvageIncomplete`): never
/// pruned, and reported by status (`SALVAGE_INCOMPLETE`), until acknowledged
/// (`Store::acknowledge_salvage`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Incomplete {
    /// The copy's file name in the project's server data.
    pub(crate) copy: String,
    /// The first read errors.
    pub(crate) errors: Vec<String>,
    /// Unix ms of the salvage.
    pub(crate) at: i64,
    #[serde(default)]
    pub(crate) acknowledged: bool,
}

const INCOMPLETE: &str = "salvageIncomplete";

/// The incomplete salvages recorded in `conn`.
pub(crate) fn incomplete(conn: &Connection) -> Vec<Incomplete> {
    db::meta(conn, INCOMPLETE)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub(crate) fn set_incomplete(conn: &Connection, all: &[Incomplete]) -> Result<(), StoreError> {
    db::set_meta(conn, INCOMPLETE, &serde_json::to_string(all)?)
}

fn open(src: &Path) -> Result<Connection, String> {
    let c = match open_read_only(src) {
        Ok(c) if c.query_row("SELECT 1", [], |_| Ok(())).is_ok() => c,
        _ => {
            // A WAL database without a writable `-shm` may refuse a read-only open.
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let c = Connection::open_with_flags(src, flags).map_err(|e| e.to_string())?;
            c.execute_batch("PRAGMA query_only = 1")
                .map_err(|e| e.to_string())?;
            c
        }
    };
    Ok(c)
}

/// The rev-0 snapshot of `main` in `src`, if it reads and belongs to `project_id`.
pub(super) fn rev0(src: &Path, project_id: &str) -> Option<Project> {
    let c = open(src).ok()?;
    let p = db::snapshot_at(&c, crate::MAIN, 0).ok()??;
    (p.id == project_id).then_some(p)
}

/// `salvagedFrom` of `src` (empty when unreadable).
pub(super) fn salvaged_from(src: &Path) -> Vec<String> {
    open(src)
        .ok()
        .and_then(|c| db::meta(&c, "salvagedFrom").ok().flatten())
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// Salvages `src` into `new` (module docs). Fails only when `new` cannot be written.
pub(super) fn run(src: &Path, new: &Connection, now: i64) -> Result<Outcome, StoreError> {
    let mut out = Outcome::default();
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let c = match open(src) {
        Ok(c) => c,
        Err(e) => {
            out.errors.push(format!("{name}: cannot open: {e}"));
            return Ok(out);
        }
    };
    let in_txs = |hash: &str| -> Result<bool, StoreError> {
        let n: i64 = new.query_row(
            "SELECT COUNT(*) FROM txs WHERE tx_hash = ?1",
            params![hash],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    };
    let mut insert = new.prepare(
        "INSERT INTO quarantine (tx_hash, branch, origin, reason, detail, record_bytes,
                                 added_at, seen_at, current)
         VALUES (?1, ?2, 'salvage', ?3, ?4, ?5, ?6, ?6, ?7) ON CONFLICT(tx_hash) DO NOTHING",
    )?;

    let (txs, errs) = rows(
        &c,
        "txs",
        "branch, seq, epoch, exported, record_bytes",
        |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, bool>(4)?,
                r.get::<_, Vec<u8>>(5)?,
            ))
        },
    );
    out.errors
        .extend(errs.into_iter().map(|e| format!("{name}: txs: {e}")));
    for (branch, seq, epoch, exported, bytes) in txs {
        if exported && branch == crate::MAIN {
            out.exported_through = out.exported_through.max(u64::try_from(seq).unwrap_or(0));
        }
        let hash = hash_bytes(&bytes);
        if in_txs(&hash)? {
            continue;
        }
        let (reason, detail) = if exported {
            out.lost_exported += 1;
            (
                "MISSING_FROM_FOLDER",
                format!(
                    "exported seq {seq} (epoch {epoch}) in {name}; the project folder no longer yields it"
                ),
            )
        } else {
            (
                "SALVAGED",
                format!("unexported seq {seq}, committed at epoch {epoch}, in {name}"),
            )
        };
        out.added +=
            u32::try_from(insert.execute(params![hash, branch, reason, detail, bytes, now, 1])?)
                .unwrap_or(0);
    }

    let (rows_q, errs) = rows(
        &c,
        "quarantine",
        "tx_hash, branch, origin, reason, detail, record_bytes, current",
        |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Vec<u8>>(6)?,
                r.get::<_, i64>(7)?,
            ))
        },
    );
    out.errors
        .extend(errs.into_iter().map(|e| format!("{name}: quarantine: {e}")));
    for (hash, branch, origin, reason, detail, bytes, current) in rows_q {
        if in_txs(&hash)? {
            continue;
        }
        let detail = if origin == "salvage" {
            detail
        } else {
            format!("{origin}: {detail}")
        };
        out.added += u32::try_from(
            insert.execute(params![hash, branch, reason, detail, bytes, now, current])?,
        )
        .unwrap_or(0);
    }

    let (ids, errs) = rows(&c, "ids", "kind, next", |r| {
        Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    });
    out.errors
        .extend(errs.into_iter().map(|e| format!("{name}: ids: {e}")));
    for (kind, next) in ids {
        new.execute(
            "INSERT INTO ids (kind, next) VALUES (?1, ?2)
             ON CONFLICT(kind) DO UPDATE SET next = MAX(next, excluded.next)",
            params![kind, next],
        )?;
    }

    // A fact about the project, not its history: its folder (each copy overwrites, as before).
    match db::meta(&c, "projectDir") {
        Ok(Some(v)) => db::set_meta(new, "projectDir", &v)?,
        Ok(None) => {}
        Err(e) => out.errors.push(format!("{name}: meta: {e}")),
    }
    if let Ok(ids) = crate::history::server_ids(&c) {
        for id in ids {
            crate::history::note_server_id(new, &id)?;
        }
    }
    out.incomplete = incomplete(&c);
    out.salvaged_from = db::meta(&c, "salvagedFrom")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    Ok(out)
}

/// The rows of `table` (`SELECT rowid, <cols>`), in rowid order, reading past damaged pages
/// (module docs). Returns them and the errors met.
fn rows<T>(
    c: &Connection,
    table: &str,
    cols: &str,
    f: impl Fn(&Row<'_>) -> rusqlite::Result<T>,
) -> (Vec<T>, Vec<String>) {
    let sql =
        format!("SELECT rowid, {cols} FROM {table} NOT INDEXED WHERE rowid > ?1 ORDER BY rowid");
    let mut out = vec![];
    let mut errors = vec![];
    // Reads from `after` on; on a failure `Err((rowid, error, row_only))`: with `row_only`
    // the row `rowid` was read but its columns do not decode (only it is skipped), otherwise
    // `rowid` is the last row read before the failure (a bad page is searched past).
    let read = |after: i64, out: &mut Vec<T>| -> Result<(), (i64, String, bool)> {
        let mut stmt = c.prepare(&sql).map_err(|e| (after, e.to_string(), false))?;
        let mut rs = stmt
            .query(params![after])
            .map_err(|e| (after, e.to_string(), false))?;
        let mut last = after;
        loop {
            match rs.next() {
                Ok(Some(row)) => {
                    let id: i64 = row.get(0).map_err(|e| (last, e.to_string(), false))?;
                    last = id;
                    let v = f(row).map_err(|e| (id, e.to_string(), true))?;
                    out.push(v);
                }
                Ok(None) => return Ok(()),
                Err(e) => return Err((last, e.to_string(), false)),
            }
        }
    };
    // Whether reading the first row after `after` works (`Some(false)`: nothing after it).
    let probe = |after: i64| -> Option<bool> {
        let mut stmt = c.prepare(&sql).ok()?;
        let mut rs = stmt.query(params![after]).ok()?;
        match rs.next() {
            Ok(Some(_)) => Some(true),
            Ok(None) => Some(false),
            Err(_) => None,
        }
    };
    let mut after = i64::MIN;
    loop {
        match read(after, &mut out) {
            Ok(()) => break,
            Err((last, e, row_only)) => {
                if errors.len() < 20 {
                    if row_only {
                        errors.push(format!("rowid {last}: {e}"));
                    } else {
                        errors.push(format!("after rowid {last}: {e}"));
                    }
                }
                if row_only {
                    // `last > after`, so this always moves on; the next row is read as is.
                    after = last;
                    continue;
                }
                // Find a readable start beyond the bad page: double the step, then narrow.
                let mut lo = last;
                let mut step: i64 = 1;
                let hi = loop {
                    let Some(at) = lo.checked_add(step) else {
                        break None;
                    };
                    match probe(at) {
                        Some(true) => break Some(at),
                        Some(false) => break None,
                        None => {
                            lo = at;
                            step = step.saturating_mul(2);
                            if step > 1 << 40 {
                                break None;
                            }
                        }
                    }
                };
                let Some(mut hi) = hi else { break };
                while hi - lo > 1 {
                    let mid = lo + (hi - lo) / 2;
                    if probe(mid) == Some(true) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                after = hi;
            }
        }
    }
    (out, errors)
}
