//! What a rebuild keeps from the database it replaces (`super`, step 4). The old file may be
//! damaged anywhere: every read is tolerated failing, and what was read before a failure is
//! kept.

use std::path::Path;

use fin_core::Project;
use rusqlite::{Connection, OpenFlags, params};

use crate::StoreError;
use crate::db::{self, hash_bytes};

/// Opens `old` for reading only (no journal-mode switch; `query_only`).
fn open(old: &Path) -> Option<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let c = Connection::open_with_flags(old, flags).ok()?;
    c.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    c.execute_batch("PRAGMA query_only = 1").ok()?;
    Some(c)
}

/// The old database's rev-0 snapshot of `main`, if it reads and belongs to `project_id`.
pub(super) fn rev0(old: &Path, project_id: &str) -> Option<Project> {
    let c = open(old)?;
    let p = db::snapshot_at(&c, crate::MAIN, 0).ok()??;
    (p.id == project_id).then_some(p)
}

/// Copies what can be read from `old` into `new` (module docs of `super`, step 4); returns
/// the quarantine rows added. Fails only when `new` cannot be written.
pub(super) fn run(old: &Path, new: &Connection, now: i64) -> Result<u32, StoreError> {
    let Some(c) = open(old) else {
        return Ok(0);
    };
    let file = old
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let in_txs = |hash: &str| -> Result<bool, StoreError> {
        Ok(new
            .query_row(
                "SELECT 1 FROM txs WHERE tx_hash = ?1",
                params![hash],
                |_| Ok(()),
            )
            .map(|()| true)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(false),
                e => Err(e),
            })?)
    };
    let mut insert = new.prepare(
        "INSERT INTO quarantine (tx_hash, branch, origin, reason, detail, record_bytes,
                                 added_at, seen_at, current)
         VALUES (?1, ?2, 'salvage', ?3, ?4, ?5, ?6, ?6, ?7) ON CONFLICT(tx_hash) DO NOTHING",
    )?;
    let mut added = 0u32;

    // Unexported records: they exist nowhere else. A table scan, not the partial index,
    // which may be the damaged part.
    for (branch, seq, epoch, bytes) in rows(
        &c,
        "SELECT branch, seq, epoch, record_bytes FROM txs NOT INDEXED WHERE exported = 0",
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        },
    ) {
        let hash = hash_bytes(&bytes);
        if in_txs(&hash)? {
            continue;
        }
        let detail = format!("unexported seq {seq}, committed at epoch {epoch}, in {file}");
        added += u32::try_from(
            insert.execute(params![hash, branch, "SALVAGED", detail, bytes, now, 1])?,
        )
        .unwrap_or(0);
    }

    // The old quarantine, with its reasons.
    for (hash, branch, origin, reason, detail, bytes, current) in rows(
        &c,
        "SELECT tx_hash, branch, origin, reason, detail, record_bytes, current FROM quarantine",
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Vec<u8>>(5)?,
                r.get::<_, i64>(6)?,
            ))
        },
    ) {
        if in_txs(&hash)? {
            continue;
        }
        added += u32::try_from(insert.execute(params![
            hash,
            branch,
            reason,
            format!("{origin}: {detail}"),
            bytes,
            now,
            current
        ])?)
        .unwrap_or(0);
    }

    // Id counters, never lowered.
    for (kind, next) in rows(&c, "SELECT kind, next FROM ids", |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    }) {
        new.execute(
            "INSERT INTO ids (kind, next) VALUES (?1, ?2)
             ON CONFLICT(kind) DO UPDATE SET next = MAX(next, excluded.next)",
            params![kind, next],
        )?;
    }

    // Where the project lives (copies), and every server id that opened it.
    if let Ok(Some(dir)) = db::meta(&c, "projectDir") {
        db::set_meta(new, "projectDir", &dir)?;
    }
    if let Ok(ids) = crate::history::server_ids(&c) {
        for id in ids {
            crate::history::note_server_id(new, &id)?;
        }
    }
    Ok(added)
}

/// The rows of `sql` that could be read before any failure.
fn rows<T>(
    c: &Connection,
    sql: &str,
    f: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Vec<T> {
    let Ok(mut stmt) = c.prepare(sql) else {
        return vec![];
    };
    let Ok(mut rows) = stmt.query([]) else {
        return vec![];
    };
    let mut out = vec![];
    while let Ok(Some(row)) = rows.next() {
        match f(row) {
            Ok(v) => out.push(v),
            Err(_) => continue,
        }
    }
    out
}
