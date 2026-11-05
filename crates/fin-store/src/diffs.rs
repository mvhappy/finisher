//! Diffs a seeded rebuild did not compute (`crate::rebuild`).
//!
//! A rebuild seeded from a folder snapshot at seq `S` stores the records `1..=S` without
//! replaying them: the snapshot is the state after `S`. Their `txs.diff` (which only a
//! replay computes) is stored as [`NOT_STORED`] and `meta` `diffsPending.<branch>` = `S`,
//! and so are the database snapshots at multiples of [`SNAPSHOT_EVERY`] below `S`. The first
//! read that needs one of those diffs ([`crate::branch::lineage::records_since`] reaching
//! below `S` on that branch, also from a descendant branch; [`crate::db::find_tx`] of such a
//! row) runs [`fill`]: one replay from the branch's start (rev 0, or its fork) through `S`
//! that writes the diffs and those snapshots, and drops the seed's own snapshot row when it
//! is not at a multiple of [`SNAPSHOT_EVERY`] and holds the replayed state. The database is
//! then exactly what a full replay would have built (tested). A seed whose state differs
//! from the replay keeps its row; `Store::verify` reports it (`replay_matches_live`).

use rusqlite::{Connection, params};

use crate::StoreError;
use crate::db::{self, SNAPSHOT_EVERY, to_i64};

/// The `txs.diff` of a row whose diff is not computed yet (JSON `null`, never a diff).
pub(crate) const NOT_STORED: &str = "null";

fn key(branch: &str) -> String {
    format!("diffsPending.{branch}")
}

/// The highest seq of `branch` whose diff may not be stored yet.
pub(crate) fn pending(conn: &Connection, branch: &str) -> Result<Option<u64>, StoreError> {
    Ok(db::meta(conn, &key(branch))?.and_then(|v| v.parse().ok()))
}

/// Records that the diffs of `branch` through `seq` are not stored.
pub(crate) fn set_pending(conn: &Connection, branch: &str, seq: u64) -> Result<(), StoreError> {
    db::set_meta(conn, &key(branch), &seq.to_string())
}

/// Computes and stores the missing diffs of `branch` (module docs), inside a savepoint, so it
/// works within the caller's transaction or alone.
pub(crate) fn fill(conn: &Connection, branch: &str) -> Result<(), StoreError> {
    let Some(through) = pending(conn, branch)? else {
        return Ok(());
    };
    conn.execute_batch("SAVEPOINT fill_diffs")?;
    let run = || -> Result<(), StoreError> {
        // A branch's own rows start after its fork, from the parent's state there.
        let start = crate::branch::lineage::lineage(conn, branch)?
            .first()
            .map_or(0, |s| s.after);
        let mut project = match db::snapshot_at(conn, branch, start)? {
            Some(p) => p,
            None if start > 0 => crate::branch::lineage::state_at(conn, branch, start)?,
            None => return Err(crate::branch_not_found(branch).into()),
        };
        let mut update = conn
            .prepare("UPDATE txs SET diff = ?3 WHERE branch = ?1 AND seq = ?2 AND diff = 'null'")?;
        for row in db::record_rows(conn, branch, start)? {
            if row.0 > through {
                break;
            }
            let applied = db::replay_row(&project, branch, &row)?;
            update.execute(params![
                branch,
                to_i64(row.0)?,
                serde_json::to_string(&applied.diff)?
            ])?;
            project = applied.project;
            if row.0.is_multiple_of(SNAPSHOT_EVERY)
                && db::snapshot_at(conn, branch, row.0)?.is_none()
            {
                db::insert_snapshot(conn, branch, row.0, &project)?;
            }
        }
        // The seed's own snapshot goes, never a branch's fork snapshot (Decision 14): a
        // child seeded at its fork seq has no rows to replay, and that row is its start.
        if through > start
            && !through.is_multiple_of(SNAPSHOT_EVERY)
            && db::snapshot_at(conn, branch, through)?.as_ref() == Some(&project)
        {
            conn.execute(
                "DELETE FROM snapshots WHERE branch = ?1 AND seq = ?2",
                params![branch, to_i64(through)?],
            )?;
        }
        conn.execute("DELETE FROM meta WHERE key = ?1", params![key(branch)])?;
        Ok(())
    };
    match run() {
        Ok(()) => {
            conn.execute_batch("RELEASE fill_diffs")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO fill_diffs; RELEASE fill_diffs");
            Err(e)
        }
    }
}
