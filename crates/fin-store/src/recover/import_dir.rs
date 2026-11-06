//! `fin project recover --import <dir>` (plan 4B Decisions 27, 29): the records of `.fseg`
//! files moved out of the project folder (one side of a `CHAIN_FORK`, by the manual
//! procedure) go into quarantine as `IMPORTED`, origin `foreign`, detail the file's path.
//! They exist nowhere else, so they stay current until their record is in `txs`; recover
//! then offers them as groups. Records the database holds are skipped; files that do not
//! decode, or belong to another project, are left alone and listed (`skipped_files`). Writes only the local database, so
//! it runs while the project is read-only.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::hash_bytes;
use crate::import::quarantine::IMPORTED;
use crate::{SkippedFile, StoreError, segment};

/// Every `.fseg` file under `dir` (recursively), sorted.
fn files(dir: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let mut out = vec![];
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).map_err(StoreError::io(&d))? {
            let e = e.map_err(StoreError::io(&d))?;
            let ty = e.file_type().map_err(StoreError::io(&e.path()))?;
            if ty.is_dir() {
                todo.push(e.path());
            } else if ty.is_file() && e.path().extension().is_some_and(|x| x == "fseg") {
                out.push(e.path());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// What [`run`] did: rows added or made current again, and the files it did not read.
#[derive(Debug, Default)]
pub(super) struct Imported {
    pub rows: usize,
    pub skipped: Vec<SkippedFile>,
}

/// Reads the segments under `dir` into quarantine.
pub(super) fn run(conn: &Connection, project_id: &str, dir: &Path) -> Result<Imported, StoreError> {
    let now = crate::commit::now_ms();
    let mut out = Imported::default();
    let mut held = conn.prepare_cached("SELECT 1 FROM txs WHERE tx_hash = ?1")?;
    let mut current = conn.prepare_cached("SELECT current FROM quarantine WHERE tx_hash = ?1")?;
    // A row already there is current again; one whose reason follows the folder's verdicts
    // becomes `IMPORTED`, since the folder no longer holds the record.
    let mut put = conn.prepare_cached(
        "INSERT INTO quarantine (tx_hash, branch, origin, reason, detail, record_bytes,
                                 added_at, seen_at, current)
         VALUES (?1, ?2, 'foreign', ?3, ?4, ?5, ?6, ?6, 1)
         ON CONFLICT(tx_hash) DO UPDATE SET
             current = 1,
             seen_at = excluded.seen_at,
             origin = CASE WHEN quarantine.reason IN ('DIVERGED_TAIL', 'SALVAGED', 'IMPORTED')
                           THEN quarantine.origin ELSE excluded.origin END,
             detail = CASE WHEN quarantine.reason IN ('DIVERGED_TAIL', 'SALVAGED', 'IMPORTED')
                           THEN quarantine.detail ELSE excluded.detail END,
             reason = CASE WHEN quarantine.reason IN ('DIVERGED_TAIL', 'SALVAGED', 'IMPORTED')
                           THEN quarantine.reason ELSE excluded.reason END",
    )?;
    for path in files(dir)? {
        let mut skip = |reason: String| {
            out.skipped.push(SkippedFile {
                path: path.to_string_lossy().into_owned(),
                reason,
            });
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                skip(format!("unreadable: {e}"));
                continue;
            }
        };
        let seg = match segment::decode(&bytes) {
            Ok(s) => s,
            Err(e) => {
                skip(format!("not a segment: {e}"));
                continue;
            }
        };
        if seg.header.project_id != project_id {
            skip(format!("another project: {}", seg.header.project_id));
            continue;
        }
        let detail = std::path::absolute(&path).unwrap_or(path);
        let detail = detail.to_string_lossy();
        for rec in &seg.records {
            let tx_hash = hash_bytes(rec);
            if held.exists(params![tx_hash])? {
                continue;
            }
            let was: Option<bool> = current
                .query_row(params![tx_hash], |r| r.get(0))
                .optional()?;
            if was != Some(true) {
                out.rows += 1;
            }
            put.execute(params![
                tx_hash,
                seg.header.branch,
                IMPORTED,
                detail,
                rec,
                now
            ])?;
        }
    }
    crate::db::Counters::raise_to_quarantine(conn)?;
    super::marks::fill(conn)?;
    Ok(out)
}
