//! The state of a branch at a past rev, from the database: its latest snapshot at or below,
//! then replay. Single-branch (4B Task 7 makes it lineage-aware).

use fin_core::Project;
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::db::{self, to_i64, to_u64};

/// The state of `branch` after `seq`. `DB_ERROR` when no snapshot is at or below `seq`.
pub(crate) fn state_at(conn: &Connection, branch: &str, seq: u64) -> Result<Project, StoreError> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, json FROM snapshots WHERE branch = ?1 AND seq <= ?2
             ORDER BY seq DESC LIMIT 1",
            params![branch, to_i64(seq)?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((base, json)) = row else {
        return Err(fin_core::CoreError::new(
            "DB_ERROR",
            format!("branch {branch:?} has no snapshot at or below seq {seq}"),
        )
        .into());
    };
    let mut project = fin_core::upcast_snapshot(serde_json::from_str(&json)?)?;
    for row in db::record_rows(conn, branch, to_u64(base)?)? {
        if row.0 > seq {
            break;
        }
        project = db::replay_row(&project, branch, &row)?.project;
    }
    Ok(project)
}
